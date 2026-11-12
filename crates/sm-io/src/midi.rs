//! MIDI output: system ports, the built-in RTP-MIDI session, and a sender
//! thread that releases messages at their scheduled instant.

use sm_core::model::RtpMidiSettings;
use sm_rtpmidi::{PeerInfo, Session, SessionConfig};
use std::collections::{BinaryHeap, HashMap};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// A destination for MIDI bytes. Implemented for system ports and test sinks.
pub trait MidiSink: Send {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String>;
}

/// Opens system MIDI ports by name. Platform specifics live behind this trait.
pub trait MidiBackend: Send {
    fn ports(&mut self) -> Vec<String>;
    fn open(&mut self, name: &str) -> Result<Box<dyn MidiSink>, String>;
}

/// Backend with no ports (used when the platform MIDI system is unavailable).
pub struct NoMidi;
impl MidiBackend for NoMidi {
    fn ports(&mut self) -> Vec<String> {
        Vec::new()
    }
    fn open(&mut self, name: &str) -> Result<Box<dyn MidiSink>, String> {
        Err(format!("MIDI port \"{name}\" not found"))
    }
}

#[cfg(feature = "midir-backend")]
mod midir_backend {
    use super::{MidiBackend, MidiSink};
    use midir::{MidiOutput, MidiOutputConnection};

    pub struct MidirBackend;

    struct Conn(MidiOutputConnection);
    impl MidiSink for Conn {
        fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
            self.0.send(bytes).map_err(|e| e.to_string())
        }
    }

    fn client() -> Result<MidiOutput, String> {
        MidiOutput::new("Show Master").map_err(|e| e.to_string())
    }

    impl MidiBackend for MidirBackend {
        fn ports(&mut self) -> Vec<String> {
            let Ok(out) = client() else { return Vec::new() };
            out.ports()
                .iter()
                .filter_map(|p| out.port_name(p).ok())
                .collect()
        }

        fn open(&mut self, name: &str) -> Result<Box<dyn MidiSink>, String> {
            let out = client()?;
            let port = out
                .ports()
                .into_iter()
                .find(|p| out.port_name(p).is_ok_and(|n| n == name))
                .ok_or_else(|| format!("MIDI port \"{name}\" not found"))?;
            let conn = out
                .connect(&port, "show-master-out")
                .map_err(|e| e.to_string())?;
            Ok(Box::new(Conn(conn)))
        }
    }
}

pub fn default_backend() -> Box<dyn MidiBackend> {
    #[cfg(feature = "midir-backend")]
    {
        Box::new(midir_backend::MidirBackend)
    }
    #[cfg(not(feature = "midir-backend"))]
    {
        Box::new(NoMidi)
    }
}

struct Scheduled {
    due: Instant,
    seq: u64,
    port: String,
    bytes: Vec<u8>,
}

impl PartialEq for Scheduled {
    fn eq(&self, o: &Self) -> bool {
        self.due == o.due && self.seq == o.seq
    }
}
impl Eq for Scheduled {}
impl PartialOrd for Scheduled {
    fn partial_cmp(&self, o: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(o))
    }
}
impl Ord for Scheduled {
    fn cmp(&self, o: &Self) -> std::cmp::Ordering {
        // BinaryHeap is a max-heap: earliest due first, FIFO within one instant.
        o.due.cmp(&self.due).then(o.seq.cmp(&self.seq))
    }
}

enum Msg {
    Send(Scheduled),
    Panic,
    /// Forget open system ports (after a device change).
    Reset,
    Quit,
}

type SharedRtp = Arc<Mutex<Option<Arc<Session>>>>;
type Errors = Arc<Mutex<Vec<String>>>;

pub const RTP_PREFIX: &str = "rtp:";
/// Destinations `osc:host:port` are OSC messages over UDP; they share the
/// sender thread so they are timed exactly like MIDI.
pub const OSC_PREFIX: &str = "osc:";

/// Owns all MIDI outputs and the sender thread.
pub struct MidiHub {
    tx: mpsc::Sender<Msg>,
    thread: Option<std::thread::JoinHandle<()>>,
    backend: Arc<Mutex<Box<dyn MidiBackend>>>,
    rtp: SharedRtp,
    rtp_cfg: Option<RtpMidiSettings>,
    errors: Errors,
    seq: u64,
    ports_cache: (Option<Instant>, Vec<String>),
    /// Backup computer in standby: nothing leaves this machine (MIDI, MTC,
    /// RTP-MIDI, OSC).
    muted: bool,
}

fn sender_loop(
    rx: mpsc::Receiver<Msg>,
    backend: Arc<Mutex<Box<dyn MidiBackend>>>,
    rtp: SharedRtp,
    errors: Errors,
) {
    let mut heap: BinaryHeap<Scheduled> = BinaryHeap::new();
    let mut open: HashMap<String, Option<Box<dyn MidiSink>>> = HashMap::new();
    let mut osc = OscSender::default();
    let report = |msg: String| {
        tracing::warn!("{msg}");
        if let Ok(mut e) = errors.lock() {
            if e.len() < 64 {
                e.push(msg);
            }
        }
    };
    let deliver = |open: &mut HashMap<String, Option<Box<dyn MidiSink>>>,
                   osc: &mut OscSender,
                   port: &str,
                   bytes: &[u8]| {
        if let Some(target) = port.strip_prefix(OSC_PREFIX) {
            if let Err(e) = osc.send(target, bytes) {
                report(format!("OSC to {target}: {e}"));
            }
            return;
        }
        if port.starts_with(RTP_PREFIX) {
            let session = rtp.lock().ok().and_then(|g| g.clone());
            match session {
                Some(s) => {
                    if let Err(e) = s.send(bytes) {
                        report(format!("RTP-MIDI send failed: {e}"));
                    }
                }
                None => report("RTP-MIDI session is not enabled".into()),
            }
            return;
        }
        if !open.contains_key(port) {
            let sink = match backend.lock() {
                Ok(mut b) => b.open(port),
                Err(_) => Err("MIDI backend unavailable".into()),
            };
            match sink {
                Ok(s) => {
                    open.insert(port.to_string(), Some(s));
                }
                Err(e) => {
                    report(e);
                    open.insert(port.to_string(), None);
                }
            }
        }
        if let Some(Some(sink)) = open.get_mut(port) {
            if let Err(e) = sink.send(bytes) {
                report(format!("MIDI send to \"{port}\" failed: {e}"));
                open.remove(port);
            }
        }
    };
    loop {
        let now = Instant::now();
        while heap.peek().is_some_and(|s| s.due <= now) {
            if let Some(s) = heap.pop() {
                deliver(&mut open, &mut osc, &s.port, &s.bytes);
            }
        }
        // Timers are coarse (and coalesced on some systems): sleep until shortly
        // before the next message, then yield-spin the last stretch.
        const SPIN: Duration = Duration::from_micros(1500);
        let wait = match heap.peek() {
            Some(s) => {
                let left = s.due.saturating_duration_since(Instant::now());
                if left <= SPIN {
                    while Instant::now() < s.due {
                        std::thread::yield_now();
                    }
                    continue;
                }
                left - SPIN
            }
            None => Duration::from_millis(500),
        };
        match rx.recv_timeout(wait) {
            Ok(Msg::Send(s)) => heap.push(s),
            Ok(Msg::Panic) => {
                heap.clear();
                let ports: Vec<String> = open
                    .iter()
                    .filter(|(_, s)| s.is_some())
                    .map(|(p, _)| p.clone())
                    .collect();
                for m in sm_core::midi::panic_messages() {
                    for p in &ports {
                        deliver(&mut open, &mut osc, p, &m);
                    }
                    deliver(&mut open, &mut osc, RTP_PREFIX, &m);
                }
            }
            Ok(Msg::Reset) => open.clear(),
            Ok(Msg::Quit) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

/// UDP socket for OSC (opened on first use) and resolved targets.
#[derive(Default)]
struct OscSender {
    v4: Option<std::net::UdpSocket>,
    v6: Option<std::net::UdpSocket>,
    resolved: HashMap<String, std::net::SocketAddr>,
}

impl OscSender {
    fn send(&mut self, target: &str, bytes: &[u8]) -> Result<(), String> {
        let addr = match self.resolved.get(target) {
            Some(a) => *a,
            None => {
                // Host names use the system resolver once (LAN names, hosts file).
                let a = target
                    .to_socket_addrs()
                    .map_err(|e| e.to_string())?
                    .next()
                    .ok_or("address not found")?;
                if self.resolved.len() > 256 {
                    self.resolved.clear();
                }
                self.resolved.insert(target.to_string(), a);
                a
            }
        };
        let slot = if addr.is_ipv4() {
            &mut self.v4
        } else {
            &mut self.v6
        };
        if slot.is_none() {
            let bind = if addr.is_ipv4() {
                "0.0.0.0:0"
            } else {
                "[::]:0"
            };
            let s = std::net::UdpSocket::bind(bind).map_err(|e| e.to_string())?;
            let _ = s.set_broadcast(true);
            *slot = Some(s);
        }
        match slot.as_ref() {
            Some(s) => s.send_to(bytes, addr).map(|_| ()).map_err(|e| {
                // Re-resolve next time (the receiver may have a new address).
                self.resolved.remove(target);
                e.to_string()
            }),
            None => Err("no socket".into()),
        }
    }
}

impl MidiHub {
    pub fn new(backend: Box<dyn MidiBackend>) -> Self {
        let (tx, rx) = mpsc::channel();
        let backend = Arc::new(Mutex::new(backend));
        let rtp: SharedRtp = Arc::new(Mutex::new(None));
        let errors: Errors = Arc::new(Mutex::new(Vec::new()));
        let (b, r, e) = (backend.clone(), rtp.clone(), errors.clone());
        let thread = std::thread::Builder::new()
            .name("sm-midi".into())
            .spawn(move || sender_loop(rx, b, r, e))
            .ok();
        Self {
            tx,
            thread,
            backend,
            rtp,
            rtp_cfg: None,
            errors,
            seq: 0,
            ports_cache: (None, Vec::new()),
            muted: false,
        }
    }

    /// Queue a message for delivery at `due` (immediately if in the past).
    /// Drop everything sent while muted (backup computer in standby).
    pub fn set_muted(&mut self, muted: bool) {
        self.muted = muted;
    }

    pub fn send_at(&mut self, due: Instant, port: &str, bytes: Vec<u8>) {
        if self.muted {
            return;
        }
        self.seq += 1;
        let _ = self.tx.send(Msg::Send(Scheduled {
            due,
            seq: self.seq,
            port: port.to_string(),
            bytes,
        }));
    }

    /// Drop everything queued and send All Notes Off / All Sound Off / Reset
    /// Controllers on all channels of every port in use.
    pub fn panic(&mut self) {
        if self.muted {
            return;
        }
        let _ = self.tx.send(Msg::Panic);
    }

    /// Available destinations: system ports plus the RTP session (if enabled).
    pub fn ports(&mut self) -> Vec<String> {
        let fresh = self
            .ports_cache
            .0
            .is_some_and(|t| t.elapsed() < Duration::from_secs(3));
        if !fresh {
            let mut ports = self
                .backend
                .lock()
                .map(|mut b| b.ports())
                .unwrap_or_default();
            if let Some(name) = self.rtp_name() {
                ports.push(format!("{RTP_PREFIX}{name}"));
            }
            self.ports_cache = (Some(Instant::now()), ports);
        }
        self.ports_cache.1.clone()
    }

    pub fn rescan(&mut self) {
        self.ports_cache.0 = None;
        let _ = self.tx.send(Msg::Reset);
    }

    fn rtp_name(&self) -> Option<String> {
        self.rtp
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|s| s.name().to_string()))
    }

    /// Start, stop or reconfigure the RTP-MIDI session to match the show.
    pub fn configure_rtp(&mut self, cfg: &RtpMidiSettings) -> Result<(), String> {
        // `advertise` only concerns mDNS (sm-mdns): toggling it keeps the session.
        let same = |c: &RtpMidiSettings| {
            RtpMidiSettings {
                advertise: cfg.advertise,
                ..c.clone()
            } == *cfg
        };
        if self.rtp_cfg.as_ref().is_some_and(same) {
            self.rtp_cfg = Some(cfg.clone());
            return Ok(());
        }
        self.rtp_cfg = Some(cfg.clone());
        self.ports_cache.0 = None;
        if let Ok(mut g) = self.rtp.lock() {
            if let Some(old) = g.take() {
                old.close();
            }
        }
        if !cfg.enabled {
            return Ok(());
        }
        let mut sc = SessionConfig::new(cfg.session_name.clone());
        sc.port = cfg.port;
        sc.journal = cfg.journal;
        let session = Session::start(sc).map_err(|e| format!("RTP-MIDI: {e}"))?;
        for peer in &cfg.connect_to {
            match resolve(peer) {
                Some(addr) => {
                    if let Err(e) = session.connect(addr) {
                        tracing::warn!("RTP-MIDI connect {peer}: {e}");
                    }
                }
                None => tracing::warn!("RTP-MIDI: cannot resolve peer '{peer}'"),
            }
        }
        if let Ok(mut g) = self.rtp.lock() {
            *g = Some(Arc::new(session));
        }
        Ok(())
    }

    pub fn rtp_connect(&self, addr: &str) -> Result<(), String> {
        let addr = resolve(addr).ok_or_else(|| format!("cannot resolve '{addr}' (use ip:port)"))?;
        let session = self
            .rtp
            .lock()
            .ok()
            .and_then(|g| g.clone())
            .ok_or("RTP-MIDI is not enabled")?;
        session.connect(addr).map_err(|e| e.to_string())
    }

    /// The running RTP-MIDI session (to receive from its peers).
    pub fn rtp_session(&self) -> Option<Arc<Session>> {
        self.rtp.lock().ok().and_then(|g| g.clone())
    }

    pub fn rtp_peers(&self) -> Vec<PeerInfo> {
        self.rtp
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|s| s.peers()))
            .unwrap_or_default()
    }

    pub fn rtp_local_port(&self) -> Option<u16> {
        self.rtp
            .lock()
            .ok()
            .and_then(|g| g.as_ref().map(|s| s.local_addr().port()))
    }

    /// Errors collected by the sender thread since the last call.
    pub fn take_errors(&self) -> Vec<String> {
        self.errors
            .lock()
            .map(|mut e| std::mem::take(&mut *e))
            .unwrap_or_default()
    }
}

fn resolve(s: &str) -> Option<SocketAddr> {
    let with_port = if s
        .rsplit_once(':')
        .is_some_and(|(_, p)| p.parse::<u16>().is_ok())
    {
        s.to_string()
    } else {
        format!("{s}:5004")
    };
    // Literal addresses only resolve locally; host names use the system resolver (LAN / hosts file).
    with_port.to_socket_addrs().ok().and_then(|mut a| a.next())
}

impl Drop for MidiHub {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        if let Ok(mut g) = self.rtp.lock() {
            if let Some(s) = g.take() {
                s.close();
            }
        }
    }
}

/// Timestamped messages captured by [`RecordingBackend`].
pub type MidiLog = Arc<Mutex<Vec<(Instant, Vec<u8>)>>>;

/// Test helper: a backend whose single port records everything it receives.
pub struct RecordingBackend {
    pub port: String,
    pub log: MidiLog,
}

struct RecordingSink(MidiLog);
impl MidiSink for RecordingSink {
    fn send(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.0
            .lock()
            .map_err(|e| e.to_string())?
            .push((Instant::now(), bytes.to_vec()));
        Ok(())
    }
}

impl MidiBackend for RecordingBackend {
    fn ports(&mut self) -> Vec<String> {
        vec![self.port.clone()]
    }
    fn open(&mut self, name: &str) -> Result<Box<dyn MidiSink>, String> {
        if name == self.port {
            Ok(Box::new(RecordingSink(self.log.clone())))
        } else {
            Err(format!("MIDI port \"{name}\" not found"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osc_is_sent_over_udp_on_time() {
        let rx = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
        rx.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
        let port = rx.local_addr().unwrap().port();
        let mut hub = MidiHub::new(Box::new(NoMidi));
        let due = Instant::now() + Duration::from_millis(30);
        let msg = sm_core::osc::OscMessage {
            host: "127.0.0.1".into(),
            port,
            address: "/cue/7/start".into(),
            args: vec![sm_core::osc::OscArg::Int(1)],
        };
        hub.send_at(
            due,
            &format!("{OSC_PREFIX}{}", msg.target()),
            msg.encode().unwrap(),
        );
        let mut buf = [0u8; 256];
        let (n, _) = rx.recv_from(&mut buf).unwrap();
        // Never early (the sender spins up to the due instant).
        assert!(Instant::now() >= due);
        assert_eq!(&buf[..n], msg.encode().unwrap().as_slice());
        // An unreachable name is reported, not fatal.
        hub.send_at(Instant::now(), "osc:no-such-host.invalid:9", vec![0; 8]);
        let deadline = Instant::now() + Duration::from_secs(5);
        while hub.take_errors().is_empty() {
            assert!(Instant::now() < deadline, "no error reported");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn scheduled_delivery_in_order_and_on_time() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut hub = MidiHub::new(Box::new(RecordingBackend {
            port: "test".into(),
            log: log.clone(),
        }));
        assert_eq!(hub.ports(), vec!["test".to_string()]);
        let t0 = Instant::now();
        hub.send_at(t0 + Duration::from_millis(60), "test", vec![0x90, 2, 1]);
        hub.send_at(t0 + Duration::from_millis(20), "test", vec![0x90, 1, 1]);
        hub.send_at(t0 + Duration::from_millis(60), "test", vec![0x90, 3, 1]);
        hub.send_at(t0, "nope", vec![0x90, 9, 1]);
        // Poll instead of assuming a fixed delay: CI machines can be slow.
        let deadline = Instant::now() + Duration::from_secs(3);
        while log.lock().unwrap().len() < 3 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        let got = log.lock().unwrap().clone();
        assert_eq!(
            got.iter().map(|g| g.1[1]).collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        // Never early. The upper bound is generous because virtualised CI
        // runners oversleep by tens of milliseconds; real jitter is measured on
        // hardware (docs/hardware-checkliste.md).
        assert!(got[0].0 >= t0 + Duration::from_millis(20));
        assert!(got[1].0 >= t0 + Duration::from_millis(60));
        assert!(
            got[0].0 < t0 + Duration::from_millis(400),
            "late by {:?}",
            got[0].0 - t0
        );
        assert!(hub.take_errors().iter().any(|e| e.contains("nope")));
        hub.panic();
        let deadline = Instant::now() + Duration::from_secs(3);
        while log.lock().unwrap().len() < 3 + 48 && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(log.lock().unwrap().len(), 3 + 48);
    }

    #[test]
    fn rtp_session_is_a_port() {
        let mut hub = MidiHub::new(Box::new(NoMidi));
        let cfg = RtpMidiSettings {
            enabled: true,
            session_name: "T".into(),
            port: 0,
            connect_to: vec![],
            journal: false,
            advertise: false,
        };
        hub.configure_rtp(&cfg).unwrap();
        assert_eq!(hub.ports(), vec!["rtp:T".to_string()]);
        assert!(hub.rtp_local_port().is_some());
        hub.configure_rtp(&RtpMidiSettings {
            enabled: false,
            ..cfg
        })
        .unwrap();
        assert!(hub.ports().is_empty());
    }
}
