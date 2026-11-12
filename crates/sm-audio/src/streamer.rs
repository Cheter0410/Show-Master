//! Background thread that keeps every voice's ring buffer topped up.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use rtrb::Producer;

use crate::renderer::VoiceShared;
use crate::source::SampleSource;

/// Frames read from one source per round-robin pass.
const CHUNK_FRAMES: usize = 4096;
/// Idle sleep when every ring is full.
const IDLE: Duration = Duration::from_millis(3);

pub(crate) struct StreamJob {
    pub(crate) id: u64,
    pub(crate) source: Box<dyn SampleSource>,
    pub(crate) producer: Producer<f32>,
    pub(crate) shared: Arc<VoiceShared>,
    pub(crate) channels: usize,
    pub(crate) preroll_frames: u64,
}

pub(crate) struct Streamer {
    jobs: Sender<StreamJob>,
    errors: Receiver<(u64, String)>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Streamer {
    pub(crate) fn spawn() -> std::io::Result<Self> {
        let (jobs, job_rx) = mpsc::channel::<StreamJob>();
        let (err_tx, errors) = mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let stop2 = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("sm-audio-streamer".into())
            .spawn(move || run(job_rx, err_tx, stop2))?;
        Ok(Self {
            jobs,
            errors,
            stop,
            thread: Some(thread),
        })
    }

    /// Register a voice; pre-filling starts immediately.
    pub(crate) fn add(&self, job: StreamJob) -> bool {
        let ok = self.jobs.send(job).is_ok();
        self.wake();
        ok
    }

    pub(crate) fn wake(&self) {
        if let Some(t) = &self.thread {
            t.thread().unpark();
        }
    }

    pub(crate) fn poll_error(&self) -> Option<(u64, String)> {
        self.errors.try_recv().ok()
    }
}

impl Drop for Streamer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.wake();
        if let Some(t) = self.thread.take() {
            if t.join().is_err() {
                tracing::error!("audio streamer thread panicked");
            }
        }
    }
}

fn run(job_rx: Receiver<StreamJob>, err_tx: Sender<(u64, String)>, stop: Arc<AtomicBool>) {
    crate::priority::boost_current_thread();
    let mut jobs: Vec<StreamJob> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();
    loop {
        loop {
            match job_rx.try_recv() {
                Ok(job) => jobs.push(job),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if stop.load(Ordering::Acquire) {
            return;
        }

        // Finished / discarded voices: drop source and producer here, far
        // away from the audio callback.
        jobs.retain(|j| !j.shared.cancel.load(Ordering::Acquire));

        // The emptiest rings first: a voice close to running dry must not
        // wait behind voices that still hold seconds of audio.
        jobs.sort_by_key(|j| {
            let cap = j.producer.buffer().capacity().max(1);
            (cap - j.producer.slots()) * 1000 / cap
        });
        let mut did_work = false;
        for job in jobs.iter_mut() {
            if job.shared.eos.load(Ordering::Relaxed) {
                continue;
            }
            did_work |= service(job, &mut scratch, &err_tx);
        }
        if !did_work {
            thread::park_timeout(IDLE);
        }
    }
}

/// Top up one voice by at most [`CHUNK_FRAMES`]. Returns whether data moved.
fn service(job: &mut StreamJob, scratch: &mut Vec<f32>, err_tx: &Sender<(u64, String)>) -> bool {
    let ch = job.channels;
    let cap_frames = job.producer.buffer().capacity() / ch;
    let free = job.producer.slots() / ch;
    if free < CHUNK_FRAMES.min(cap_frames / 2).max(1) {
        return false;
    }
    let want = free.min(CHUNK_FRAMES);
    if scratch.len() < want * ch {
        scratch.resize(want * ch, 0.0);
    }
    let buf = &mut scratch[..want * ch];
    let source = &mut job.source;
    let result = catch_unwind(AssertUnwindSafe(|| {
        let n = source.read(buf);
        let err = if n == 0 { source.take_error() } else { None };
        (n, err)
    }));
    let (frames, error) = match result {
        Ok((n, err)) => (n.min(want), err),
        Err(_) => (0, Some("source panicked while reading".to_string())),
    };
    if let Some(message) = error {
        tracing::warn!(voice = job.id, %message, "audio source error, ending stream");
        // A closed channel only means the controller is gone.
        let _ = err_tx.send((job.id, message));
    }
    if frames == 0 {
        job.shared.primed.store(true, Ordering::Release);
        job.shared.eos.store(true, Ordering::Release);
        return true;
    }
    let samples = frames * ch;
    match job.producer.write_chunk(samples) {
        Ok(mut chunk) => {
            let (a, b) = chunk.as_mut_slices();
            let split = a.len();
            a.copy_from_slice(&buf[..split]);
            b.copy_from_slice(&buf[split..samples]);
            chunk.commit_all();
        }
        Err(_) => return false, // cannot happen: `free` was checked above
    }
    let written = job.shared.written.load(Ordering::Relaxed) + frames as u64;
    job.shared.written.store(written, Ordering::Release);
    if written >= job.preroll_frames.min(cap_frames as u64) {
        job.shared.primed.store(true, Ordering::Release);
    }
    true
}
