//! Proof that the real-time path never touches the heap.
//!
//! A counting global allocator records every allocation, reallocation and
//! deallocation made **on the current thread** while a guard is active. The
//! test wraps each `Renderer::render` call in such a guard while voices are
//! added, started, faded, paused, stopped, underrun and finished.
//!
//! `unsafe` is denied workspace-wide; implementing `GlobalAlloc` is the one
//! place that cannot do without it, so it is allowed for this file only.
#![allow(unsafe_code)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use common::{noise_i16, wait_primed, write_wav_i16, TempDir};
use sm_audio::{
    engine_with_generators, AudioEvent, EngineConfig, FadeCurve, FileSource, FileSourceOpts,
    MemorySource, RtGenerator, SampleSource, VoiceParams,
};

thread_local! {
    static COUNTING: Cell<bool> = const { Cell::new(false) };
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
    static DEALLOCS: Cell<u64> = const { Cell::new(0) };
}

struct CountingAlloc;

fn note(counter: &'static std::thread::LocalKey<Cell<u64>>) {
    // `try_with`: the allocator may be called during thread teardown.
    let counting = COUNTING.try_with(|c| c.get()).unwrap_or(false);
    if counting {
        let _ = counter.try_with(|c| c.set(c.get() + 1));
    }
}

// SAFETY: defers to the system allocator; the bookkeeping only touches
// const-initialised thread-local `Cell`s and never allocates itself.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note(&ALLOCS);
        unsafe { System.alloc(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        note(&DEALLOCS);
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note(&ALLOCS);
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note(&ALLOCS);
        note(&DEALLOCS);
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Run `f` with allocation counting on; returns (allocs, deallocs).
fn counted(f: impl FnOnce()) -> (u64, u64) {
    let a0 = ALLOCS.with(|c| c.get());
    let d0 = DEALLOCS.with(|c| c.get());
    COUNTING.with(|c| c.set(true));
    f();
    COUNTING.with(|c| c.set(false));
    (
        ALLOCS.with(|c| c.get()) - a0,
        DEALLOCS.with(|c| c.get()) - d0,
    )
}

#[test]
fn the_counter_itself_works() {
    let (a, d) = counted(|| {
        let v: Vec<u8> = Vec::with_capacity(100);
        drop(v);
    });
    assert_eq!((a, d), (1, 1));
    let (a, d) = counted(|| {
        let x = [0u8; 64];
        std::hint::black_box(&x);
    });
    assert_eq!((a, d), (0, 0));
}

struct Tone(f32);

impl RtGenerator for Tone {
    fn render(&mut self, _start: u64, out: &mut [f32], channels: usize) {
        for f in out.chunks_exact_mut(channels) {
            self.0 = (self.0 + 0.01) % 1.0;
            f[channels - 1] = self.0 * 0.1;
        }
    }
}

/// Never delivers more than `limit` frames until released → underrun.
struct Starving {
    sent: usize,
    limit: usize,
    release: Arc<AtomicBool>,
}

impl SampleSource for Starving {
    fn channels(&self) -> usize {
        1
    }
    fn total_frames(&self) -> Option<u64> {
        None
    }
    fn read(&mut self, buf: &mut [f32]) -> usize {
        if self.sent >= self.limit {
            if self.release.load(Ordering::Acquire) {
                return 0;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
            // deliver a trickle so `read` never reports end of stream
            buf[0] = 0.1;
            self.sent += 1;
            return 1;
        }
        let n = buf.len().min(self.limit - self.sent);
        buf[..n].fill(0.1);
        self.sent += n;
        n
    }
}

#[test]
fn render_never_allocates_or_frees() {
    const OUTS: usize = 8;
    let dir = TempDir::new("noalloc");
    let mut cfg = EngineConfig::new(48_000, OUTS);
    cfg.preroll_frames = 2000;
    let (mut ctl, mut r) = engine_with_generators(cfg, vec![Box::new(Tone(0.0))]);

    let mut total_allocs = 0u64;
    let mut total_deallocs = 0u64;
    let mut out = vec![0.0f32; 1024 * OUTS];
    let mut rendered = 0usize;
    let mut blocks = 0usize;
    let sizes = [64usize, 333, 512, 1, 1000, 127];
    let mut render_some = |r: &mut sm_audio::Renderer, frames: usize| {
        let mut left = frames;
        while left > 0 {
            let n = sizes[blocks % sizes.len()].min(left);
            let buf = &mut out[..n * OUTS];
            let (a, d) = counted(|| r.render(buf, OUTS));
            total_allocs += a;
            total_deallocs += d;
            blocks += 1;
            left -= n;
            rendered += n;
        }
    };

    // --- a mix of voice kinds, all prepared up front
    let mut voices = Vec::new();
    for i in 0..12usize {
        let v = if i % 3 == 0 {
            let p = dir.path(&format!("f{i}.wav"));
            write_wav_i16(&p, 48_000, 2, &noise_i16(2 * 24_000, i as u32 + 1));
            let mut o = FileSourceOpts::new(48_000);
            o.loop_count = 4;
            ctl.prepare_voice(
                Box::new(FileSource::open(&p, o).unwrap()),
                VoiceParams {
                    gain: 0.1,
                    matrix: vec![
                        (0, (i % OUTS) as u16, 1.0),
                        (1, ((i + 1) % OUTS) as u16, 1.0),
                    ],
                },
            )
            .unwrap()
        } else {
            let frames = 3000 + i * 4000;
            ctl.prepare_voice(
                Box::new(MemorySource::new(vec![0.25; frames * 2], 2)),
                VoiceParams {
                    gain: 0.1,
                    matrix: vec![
                        (0, (i % OUTS) as u16, 1.0),
                        (1, ((i + 3) % OUTS) as u16, 0.5),
                    ],
                },
            )
            .unwrap()
        };
        voices.push(v);
    }
    let release = Arc::new(AtomicBool::new(false));
    let starving = ctl
        .prepare_voice(
            Box::new(Starving {
                sent: 0,
                limit: 2500,
                release: Arc::clone(&release),
            }),
            VoiceParams::default(),
        )
        .unwrap();
    wait_primed(&ctl, &voices);
    wait_primed(&ctl, &[starving]);

    // AddVoice commands are consumed here (no allocation: the boxes were
    // made by `prepare_voice` on this, the control, side).
    render_some(&mut r, 100);

    // --- timestamped starts, fades, pauses, stops, all mid-block
    let t = ctl.clock();
    for (i, v) in voices.iter().enumerate() {
        let curve = [FadeCurve::Linear, FadeCurve::EqualPower, FadeCurve::SCurve][i % 3];
        ctl.start(*v, t + 37 + i as u64 * 101, (i as u64 % 4) * 700, curve)
            .unwrap();
    }
    ctl.start(starving, t + 5, 0, FadeCurve::Linear).unwrap();
    ctl.fade(
        voices[0],
        t + 900,
        3000,
        FadeCurve::EqualPower,
        Some(0.5),
        &[(0, 5, 0.7), (1, 2, 0.3)],
    )
    .unwrap();
    let many: Vec<(u16, u16, f32)> = (0..OUTS as u16)
        .flat_map(|o| [(0, o, 0.2), (1, o, 0.1)])
        .collect();
    ctl.fade(voices[3], t + 1500, 5000, FadeCurve::SCurve, None, &many)
        .unwrap();
    ctl.pause(voices[6], t + 2000).unwrap();
    ctl.resume(voices[6], t + 9000).unwrap();
    ctl.stop(voices[9], t + 4000, 2000, FadeCurve::EqualPower, false)
        .unwrap();
    ctl.stop(voices[0], t + 12_000, 0, FadeCurve::Linear, false)
        .unwrap();
    ctl.stop(voices[3], t + 13_000, 0, FadeCurve::Linear, true)
        .unwrap();
    ctl.set_master_gain_at(t + 6000, 0.7, 4000, FadeCurve::Linear)
        .unwrap();
    ctl.set_output_gain(2, 0.0, 480).unwrap();

    // Render ~1 s without polling: finished voices pile up in the return
    // queue, underruns happen on the starving voice.
    render_some(&mut r, 48_000);
    assert!(ctl.underruns() >= 1, "the starving voice underran");
    assert!(ctl.active_voices() >= 1);

    // --- now interleave control traffic and rendering
    let mut ended = 0;
    let mut underrun_events = 0;
    for round in 0..40 {
        for e in ctl.poll_events() {
            match e {
                AudioEvent::VoiceEnded { .. } => ended += 1,
                AudioEvent::Underrun { .. } => underrun_events += 1,
                AudioEvent::SourceError { .. } => panic!("unexpected source error"),
            }
        }
        if round == 5 {
            release.store(true, Ordering::Release);
        }
        if round == 10 {
            ctl.stop_all(ctl.clock() + 777, 4800, false).unwrap();
        }
        // short voices come and go while everything else runs
        let v = ctl
            .prepare_voice(
                Box::new(MemorySource::new(vec![0.1; 1500], 1)),
                VoiceParams {
                    gain: 1.0,
                    matrix: vec![(0, (round % OUTS) as u16, 0.3)],
                },
            )
            .unwrap();
        wait_primed(&ctl, &[v]);
        ctl.start(v, ctl.clock() + 13, 100, FadeCurve::Linear)
            .unwrap();
        render_some(&mut r, 4800);
    }
    for e in ctl.poll_events() {
        if matches!(e, AudioEvent::VoiceEnded { .. }) {
            ended += 1;
        }
    }

    println!(
        "rendered {rendered} frames in {blocks} blocks: {total_allocs} allocations, \
         {total_deallocs} deallocations inside render; {ended} voices ended, \
         {underrun_events} underrun event(s)"
    );
    assert!(rendered > 200_000);
    assert_eq!(
        ended,
        12 + 1 + 40,
        "every voice came back to the control side"
    );
    assert!(underrun_events >= 1);
    assert_eq!(ctl.voice_count(), 0);
    assert_eq!(total_allocs, 0, "render allocated");
    assert_eq!(total_deallocs, 0, "render freed memory");
}
