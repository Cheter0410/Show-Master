//! Keep the threads that feed the audio from being starved.
//!
//! The device callback runs on the system's real-time audio thread. The
//! threads that decode files into the voice ring buffers (streamer) and that
//! schedule cues (engine) are ordinary threads: on a busy or power-saving
//! system - macOS throttles timers and background apps ("App Nap") - they
//! can be woken far too late, the rings run dry and playback stutters no
//! matter how large the device buffer is. So:
//!
//! * those threads ask for a high scheduling class
//!   ([`boost_current_thread`]): macOS QoS "user interactive", Windows
//!   `THREAD_PRIORITY_HIGHEST`, Linux a lower nice value when permitted;
//! * the process tells macOS it is doing latency-critical, user-initiated
//!   work for its whole life ([`keep_awake`]): no App Nap, no timer
//!   coalescing, no sleep while a show runs.

#![allow(unsafe_code)]

/// Raise the calling thread's scheduling priority (best effort, never fails).
pub fn boost_current_thread() {
    #[cfg(target_os = "macos")]
    {
        // QOS_CLASS_USER_INTERACTIVE = 0x21
        // SAFETY: plain libc call on the current thread with a valid class.
        let r = unsafe {
            libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0)
        };
        if r != 0 {
            tracing::debug!("thread QoS not raised ({r})");
        }
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // A negative nice value needs CAP_SYS_NICE / rtprio limits; harmless otherwise.
        // SAFETY: plain libc calls on the current thread.
        unsafe {
            let tid = libc::syscall(libc::SYS_gettid) as libc::id_t;
            let _ = libc::setpriority(libc::PRIO_PROCESS, tid, -10);
        }
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_HIGHEST,
        };
        // SAFETY: the pseudo handle of the current thread is always valid.
        unsafe {
            SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_HIGHEST);
        }
    }
}

/// Ask the system not to throttle this process (macOS App Nap, timer
/// coalescing, idle sleep). Call once at start; lasts for the process.
pub fn keep_awake() {
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSActivityOptions, NSProcessInfo, NSString};
        let info = NSProcessInfo::processInfo();
        let options = NSActivityOptions::UserInitiated | NSActivityOptions::LatencyCritical;
        let reason = NSString::from_str("Show playback: audio, timecode and cue timing");
        let token = info.beginActivityWithOptions_reason(options, &reason);
        // The activity ends when the token is released: keep it forever.
        std::mem::forget(token);
        tracing::info!("macOS App Nap disabled for this process");
    }
}

/// CPU time this process has used so far (all threads), in seconds.
pub fn process_cpu_seconds() -> Option<f64> {
    #[cfg(unix)]
    {
        // SAFETY: getrusage fills the zeroed struct for RUSAGE_SELF.
        let mut u: libc::rusage = unsafe { std::mem::zeroed() };
        let r = unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut u) };
        if r != 0 {
            return None;
        }
        let t = |tv: libc::timeval| tv.tv_sec as f64 + tv.tv_usec as f64 / 1e6;
        Some(t(u.ru_utime) + t(u.ru_stime))
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::Foundation::FILETIME;
        use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let zero = FILETIME {
            dwLowDateTime: 0,
            dwHighDateTime: 0,
        };
        let (mut c, mut e, mut k, mut u) = (zero, zero, zero, zero);
        // SAFETY: the pseudo handle of the current process is always valid.
        let ok = unsafe { GetProcessTimes(GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) };
        if ok == 0 {
            return None;
        }
        let secs = |f: FILETIME| {
            ((u64::from(f.dwHighDateTime) << 32) | u64::from(f.dwLowDateTime)) as f64 / 1e7
        };
        Some(secs(k) + secs(u))
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// Process CPU load as a fraction of one core, measured between calls.
#[derive(Default)]
pub struct CpuMeter {
    last: Option<(std::time::Instant, f64)>,
    value: f32,
}

impl CpuMeter {
    /// Update (at most every half second) and return the load (1.0 = one core).
    pub fn sample(&mut self) -> f32 {
        let now = std::time::Instant::now();
        let Some(cpu) = process_cpu_seconds() else {
            return 0.0;
        };
        match self.last {
            Some((t, c)) if now.duration_since(t).as_secs_f64() >= 0.5 => {
                let wall = now.duration_since(t).as_secs_f64();
                self.value = ((cpu - c) / wall).max(0.0) as f32;
                self.last = Some((now, cpu));
            }
            None => self.last = Some((now, cpu)),
            _ => {}
        }
        self.value
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn process_cpu_moves() {
        let a = super::process_cpu_seconds().unwrap_or(0.0);
        let mut x = 0u64;
        for i in 0..20_000_000u64 {
            x = x.wrapping_mul(31).wrapping_add(i);
        }
        std::hint::black_box(x);
        let b = super::process_cpu_seconds().unwrap_or(0.0);
        assert!(b > a, "{a} -> {b}");
    }
}
