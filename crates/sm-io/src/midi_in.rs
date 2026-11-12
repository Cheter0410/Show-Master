//! MIDI input: system ports (midir) and RTP-MIDI peers.
//!
//! Ports are opened and closed on a thread of their own (`sm-midi-in`), so
//! neither the engine thread nor the window ever waits for a MIDI driver.
//! Device callbacks only timestamp the message and put it into a channel;
//! the runtime takes it from there on its own thread.

use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// One complete MIDI message received on `port` at `at`.
#[derive(Clone, Debug, PartialEq)]
pub struct InboundMidi {
    /// System port name, or `rtp:<session>` for RTP-MIDI peers.
    pub port: String,
    pub bytes: Vec<u8>,
    pub at: Instant,
}

pub type InboundTx = mpsc::Sender<InboundMidi>;

/// An open input port; dropping it closes the port.
pub trait MidiInputPort: Send {}

/// Opens system MIDI inputs. Platform specifics live behind this trait.
pub trait MidiInputBackend: Send {
    fn ports(&mut self) -> Vec<String>;
    /// Open `name`; every message goes to `tx` (tagged with `name`).
    fn open(&mut self, name: &str, tx: InboundTx) -> Result<Box<dyn MidiInputPort>, String>;
}

/// No system MIDI inputs.
pub struct NoMidiInput;

impl MidiInputBackend for NoMidiInput {
    fn ports(&mut self) -> Vec<String> {
        Vec::new()
    }
    fn open(&mut self, name: &str, _tx: InboundTx) -> Result<Box<dyn MidiInputPort>, String> {
        Err(format!("MIDI input \"{name}\" not found"))
    }
}

/// Clock and active sensing are never needed and would flood the channel.
pub fn is_noise(bytes: &[u8]) -> bool {
    matches!(bytes.first(), Some(0xF8 | 0xFE) | None)
}

#[cfg(feature = "midir-backend")]
mod midir_input {
    use super::{is_noise, InboundMidi, InboundTx, MidiInputBackend, MidiInputPort};
    use midir::{Ignore, MidiInput, MidiInputConnection};
    use std::time::Instant;

    #[derive(Default)]
    pub struct MidirInput {
        lister: Option<MidiInput>,
        /// Last failed attempt to reach the MIDI system (retried slowly).
        failed: Option<Instant>,
    }

    struct Conn(#[allow(dead_code)] MidiInputConnection<()>);
    impl MidiInputPort for Conn {}

    fn client() -> Result<MidiInput, String> {
        let mut c = MidiInput::new("Show Master").map_err(|e| e.to_string())?;
        c.ignore(Ignore::None);
        Ok(c)
    }

    impl MidiInputBackend for MidirInput {
        fn ports(&mut self) -> Vec<String> {
            let retry = self
                .failed
                .is_none_or(|t| t.elapsed() >= std::time::Duration::from_secs(30));
            if self.lister.is_none() && retry {
                self.lister = client().ok();
                self.failed = self.lister.is_none().then(Instant::now);
            }
            let Some(c) = &self.lister else {
                return Vec::new();
            };
            c.ports()
                .iter()
                .filter_map(|p| c.port_name(p).ok())
                .collect()
        }

        fn open(&mut self, name: &str, tx: InboundTx) -> Result<Box<dyn MidiInputPort>, String> {
            let c = client()?;
            let port = c
                .ports()
                .into_iter()
                .find(|p| c.port_name(p).is_ok_and(|n| n == name))
                .ok_or_else(|| format!("MIDI input \"{name}\" not found"))?;
            let tag = name.to_string();
            let conn = c
                .connect(
                    &port,
                    "show-master-in",
                    move |_, bytes, _| {
                        if !is_noise(bytes) {
                            let _ = tx.send(InboundMidi {
                                port: tag.clone(),
                                bytes: bytes.to_vec(),
                                at: Instant::now(),
                            });
                        }
                    },
                    (),
                )
                .map_err(|e| e.to_string())?;
            Ok(Box::new(Conn(conn)))
        }
    }
}

pub fn default_input_backend() -> Box<dyn MidiInputBackend> {
    #[cfg(feature = "midir-backend")]
    {
        Box::new(midir_input::MidirInput::default())
    }
    #[cfg(not(feature = "midir-backend"))]
    {
        Box::new(NoMidiInput)
    }
}

/// Which system input ports to keep open.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum WantedInputs {
    #[default]
    None,
    /// Every port except "Through" loopbacks, plus `extra`.
    All {
        extra: Vec<String>,
    },
    Named(Vec<String>),
}

enum Cmd {
    Want(WantedInputs),
    Quit,
}

#[derive(Default)]
struct Shared {
    available: Vec<String>,
    open: Vec<String>,
    errors: Vec<String>,
}

/// Owns the input ports and the thread that manages them.
pub struct MidiInputHub {
    cmd: mpsc::Sender<Cmd>,
    rx: mpsc::Receiver<InboundMidi>,
    tx: InboundTx,
    shared: Arc<Mutex<Shared>>,
    wanted: WantedInputs,
    thread: Option<std::thread::JoinHandle<()>>,
}

const RESCAN: Duration = Duration::from_secs(2);

fn through(name: &str) -> bool {
    name.to_ascii_lowercase().contains("through")
}

fn manager(
    mut backend: Box<dyn MidiInputBackend>,
    cmd: mpsc::Receiver<Cmd>,
    tx: InboundTx,
    shared: Arc<Mutex<Shared>>,
) {
    let mut wanted = WantedInputs::None;
    let mut open: HashMap<String, Box<dyn MidiInputPort>> = HashMap::new();
    let mut failed: HashSet<String> = HashSet::new();
    loop {
        let available = backend.ports();
        let mut want: Vec<String> = match &wanted {
            WantedInputs::None => Vec::new(),
            WantedInputs::All { extra } => available
                .iter()
                .filter(|p| !through(p) || extra.contains(p))
                .cloned()
                .chain(extra.iter().filter(|e| available.contains(e)).cloned())
                .collect(),
            WantedInputs::Named(names) => names.clone(),
        };
        want.sort();
        want.dedup();
        // Close what is no longer wanted or no longer there (device unplugged).
        open.retain(|name, _| want.contains(name) && available.contains(name));
        let mut errors = Vec::new();
        for name in &want {
            if open.contains_key(name) {
                continue;
            }
            if !available.contains(name) {
                if failed.insert(name.clone()) {
                    errors.push(format!("MIDI input \"{name}\" not found"));
                }
                continue;
            }
            match backend.open(name, tx.clone()) {
                Ok(p) => {
                    if failed.remove(name) {
                        errors.push(format!("MIDI input \"{name}\" connected"));
                    }
                    open.insert(name.clone(), p);
                }
                Err(e) => {
                    if failed.insert(name.clone()) {
                        errors.push(format!("MIDI input \"{name}\": {e}"));
                    }
                }
            }
        }
        failed.retain(|f| want.contains(f));
        if let Ok(mut s) = shared.lock() {
            s.available = available;
            let mut names: Vec<String> = open.keys().cloned().collect();
            names.sort();
            s.open = names;
            if s.errors.len() < 64 {
                s.errors.extend(errors);
            }
        }
        match cmd.recv_timeout(RESCAN) {
            Ok(Cmd::Want(w)) => wanted = w,
            Ok(Cmd::Quit) | Err(RecvTimeoutError::Disconnected) => return,
            Err(RecvTimeoutError::Timeout) => {}
        }
    }
}

impl MidiInputHub {
    pub fn new(backend: Box<dyn MidiInputBackend>) -> Self {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let (t, s) = (tx.clone(), shared.clone());
        let thread = std::thread::Builder::new()
            .name("sm-midi-in".into())
            .spawn(move || manager(backend, cmd_rx, t, s))
            .map_err(|e| tracing::error!("cannot start MIDI input thread: {e}"))
            .ok();
        Self {
            cmd: cmd_tx,
            rx,
            tx,
            shared,
            wanted: WantedInputs::None,
            thread,
        }
    }

    /// Change the set of open ports (applied on the input thread).
    pub fn set_wanted(&mut self, wanted: WantedInputs) {
        if wanted != self.wanted {
            self.wanted = wanted.clone();
            let _ = self.cmd.send(Cmd::Want(wanted));
        }
    }

    /// A sender for other sources of incoming MIDI (RTP-MIDI peers).
    pub fn sender(&self) -> InboundTx {
        self.tx.clone()
    }

    /// Messages received since the last call (never blocks).
    pub fn drain(&self) -> Vec<InboundMidi> {
        self.rx.try_iter().take(4096).collect()
    }

    /// System input ports seen by the last scan.
    pub fn available(&self) -> Vec<String> {
        self.shared
            .lock()
            .map(|s| s.available.clone())
            .unwrap_or_default()
    }

    /// Ports currently open.
    pub fn open_ports(&self) -> Vec<String> {
        self.shared
            .lock()
            .map(|s| s.open.clone())
            .unwrap_or_default()
    }

    /// Connection problems (and recoveries) since the last call.
    pub fn take_messages(&self) -> Vec<String> {
        self.shared
            .lock()
            .map(|mut s| std::mem::take(&mut s.errors))
            .unwrap_or_default()
    }
}

impl Drop for MidiInputHub {
    fn drop(&mut self) {
        let _ = self.cmd.send(Cmd::Quit);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

// ---------------------------------------------------------------------------
// Test backend
// ---------------------------------------------------------------------------

type Senders = Arc<Mutex<HashMap<String, InboundTx>>>;

/// Test helper: virtual input ports fed through [`FakeMidiInputFeed`].
pub struct FakeMidiInput {
    ports: Arc<Mutex<Vec<String>>>,
    senders: Senders,
}

/// Plays messages into the ports of a [`FakeMidiInput`] (and plugs / unplugs them).
#[derive(Clone)]
pub struct FakeMidiInputFeed {
    ports: Arc<Mutex<Vec<String>>>,
    senders: Senders,
}

impl FakeMidiInput {
    pub fn new(ports: &[&str]) -> (Self, FakeMidiInputFeed) {
        let ports = Arc::new(Mutex::new(ports.iter().map(|p| p.to_string()).collect()));
        let senders: Senders = Arc::new(Mutex::new(HashMap::new()));
        (
            Self {
                ports: ports.clone(),
                senders: senders.clone(),
            },
            FakeMidiInputFeed { ports, senders },
        )
    }
}

struct FakePort {
    name: String,
    senders: Senders,
}
impl MidiInputPort for FakePort {}
impl Drop for FakePort {
    fn drop(&mut self) {
        if let Ok(mut s) = self.senders.lock() {
            s.remove(&self.name);
        }
    }
}

impl MidiInputBackend for FakeMidiInput {
    fn ports(&mut self) -> Vec<String> {
        self.ports.lock().map(|p| p.clone()).unwrap_or_default()
    }
    fn open(&mut self, name: &str, tx: InboundTx) -> Result<Box<dyn MidiInputPort>, String> {
        if !self.ports().iter().any(|p| p == name) {
            return Err(format!("MIDI input \"{name}\" not found"));
        }
        if let Ok(mut s) = self.senders.lock() {
            s.insert(name.to_string(), tx);
        }
        Ok(Box::new(FakePort {
            name: name.to_string(),
            senders: self.senders.clone(),
        }))
    }
}

impl FakeMidiInputFeed {
    /// Deliver `bytes` on `port`; false when that port is not open.
    pub fn send(&self, port: &str, bytes: &[u8]) -> bool {
        self.send_at(port, bytes, Instant::now())
    }

    pub fn send_at(&self, port: &str, bytes: &[u8], at: Instant) -> bool {
        let tx = self.senders.lock().ok().and_then(|s| s.get(port).cloned());
        tx.is_some_and(|tx| {
            tx.send(InboundMidi {
                port: port.to_string(),
                bytes: bytes.to_vec(),
                at,
            })
            .is_ok()
        })
    }

    pub fn is_open(&self, port: &str) -> bool {
        self.senders
            .lock()
            .map(|s| s.contains_key(port))
            .unwrap_or(false)
    }

    pub fn set_ports(&self, ports: &[&str]) {
        if let Ok(mut p) = self.ports.lock() {
            *p = ports.iter().map(|p| p.to_string()).collect();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !f() {
            assert!(Instant::now() < deadline, "timed out: {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn opens_wanted_ports_and_delivers() {
        let (backend, feed) = FakeMidiInput::new(&["Keys", "Desk", "Midi Through Port-0"]);
        let mut hub = MidiInputHub::new(Box::new(backend));
        wait("scan", || hub.available().len() == 3);
        assert!(hub.open_ports().is_empty());
        hub.set_wanted(WantedInputs::All { extra: vec![] });
        wait("all open", || hub.open_ports() == vec!["Desk", "Keys"]);
        assert!(!feed.is_open("Midi Through Port-0"));
        assert!(feed.send("Keys", &[0x90, 60, 100]));
        wait("message", || {
            hub.drain()
                .iter()
                .any(|m| m.port == "Keys" && m.bytes == [0x90, 60, 100])
        });
        hub.set_wanted(WantedInputs::Named(vec!["Desk".into(), "Gone".into()]));
        wait("named", || hub.open_ports() == vec!["Desk"]);
        assert!(!feed.is_open("Keys"));
        wait("error", || {
            hub.take_messages()
                .iter()
                .any(|m| m.contains("\"Gone\" not found"))
        });
        // Unplugged: closed; back: reopened and reported.
        feed.set_ports(&["Keys"]);
        wait("closed", || hub.open_ports().is_empty());
        feed.set_ports(&["Keys", "Desk"]);
        wait("reopened", || hub.open_ports() == vec!["Desk"]);
        hub.set_wanted(WantedInputs::None);
        wait("none", || hub.open_ports().is_empty());
        // Other sources share the channel.
        hub.sender()
            .send(InboundMidi {
                port: "rtp:x".into(),
                bytes: vec![0xC0, 1],
                at: Instant::now(),
            })
            .unwrap();
        assert_eq!(hub.drain().len(), 1);
    }

    #[test]
    fn noise_filter() {
        assert!(is_noise(&[0xF8]));
        assert!(is_noise(&[0xFE]));
        assert!(!is_noise(&[0xF1, 0x10]));
    }
}
