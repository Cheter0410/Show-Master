//! The RTP-MIDI session: sockets, threads, peer state machine.

use crate::applemidi::{self, Exchange, SessionPacket, Sync, PROTOCOL_VERSION};
use crate::journal::JournalHistory;
use crate::recovery::{self, ReceiverState};
use crate::rtp::{self, RtpHeader, SysexAssembler};
use crate::Error;
use std::hash::{BuildHasher, Hasher};
use std::io::ErrorKind;
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, info, trace, warn};

/// Largest MIDI list put into one packet; keeps packets well below a 1500 byte MTU.
const MAX_LIST_BYTES: usize = 1200;
/// Granularity of the timer wheel and of socket shutdown checks.
const TICK: Duration = Duration::from_millis(20);
/// Delay between receiving MIDI and acknowledging it with `RS`.
const FEEDBACK_DELAY: Duration = Duration::from_millis(250);
/// Number of unanswered `CK0` after which an initiated peer counts as lost.
const MAX_MISSED_SYNCS: u32 = 3;
/// A responder-side peer must send its data-port invitation within this time.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(5);

/// Configuration of a [`Session`]. Use struct update syntax with
/// [`SessionConfig::default`] or start from [`SessionConfig::new`].
#[derive(Debug, Clone)]
pub struct SessionConfig {
    /// Session name announced to peers.
    pub name: String,
    /// Control port; the data port is `port + 1`. `0` picks a free pair.
    /// Default 5004.
    pub port: u16,
    /// Local address to bind. Default `0.0.0.0`.
    pub bind_addr: IpAddr,
    /// Attach a recovery journal (chapters P, C, N) to outgoing packets.
    /// Default `false` (J=0).
    pub journal: bool,
    /// Fixed SSRC; `None` picks a random one.
    pub ssrc: Option<u32>,
    /// Maximum number of peers (initiated + accepted). Further invitations
    /// are rejected with `NO`. Default 16.
    pub max_peers: usize,
    /// Interval between invitation retries. Default 1 s.
    pub invite_retry_interval: Duration,
    /// Invitations sent per port before the peer is marked failed. Default 10.
    pub invite_max_tries: u32,
    /// Re-invite failed or lost peers that were added with
    /// [`Session::connect`]. Default `true`.
    pub auto_reconnect: bool,
    /// Pause before a failed peer is invited again. Default 5 s.
    pub reconnect_interval: Duration,
    /// Number of quick clock syncs after connecting. Default 6.
    pub sync_fast_count: u32,
    /// Interval of the quick clock syncs. Default 500 ms.
    pub sync_fast_interval: Duration,
    /// Regular clock sync interval (initiator role). After three unanswered
    /// syncs the peer is considered lost. Default 10 s.
    pub sync_interval: Duration,
    /// A peer that invited us is dropped after this long without any packet
    /// from it. Default 60 s.
    pub peer_timeout: Duration,
}

impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            name: "Show Master".to_owned(),
            port: 5004,
            bind_addr: IpAddr::V4(Ipv4Addr::UNSPECIFIED),
            journal: false,
            ssrc: None,
            max_peers: 16,
            invite_retry_interval: Duration::from_secs(1),
            invite_max_tries: 10,
            auto_reconnect: true,
            reconnect_interval: Duration::from_secs(5),
            sync_fast_count: 6,
            sync_fast_interval: Duration::from_millis(500),
            sync_interval: Duration::from_secs(10),
            peer_timeout: Duration::from_secs(60),
        }
    }
}

impl SessionConfig {
    /// Default configuration with the given session name.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
}

/// Connection state of a peer as shown to the user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeerState {
    /// Invitation handshake in progress.
    Connecting,
    /// Handshake complete; MIDI flows.
    Connected,
    /// Invitation rejected, unanswered, or the peer was lost. With
    /// [`SessionConfig::auto_reconnect`] the peer returns to `Connecting`
    /// after [`SessionConfig::reconnect_interval`].
    Failed,
}

/// Snapshot of one peer for display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerInfo {
    /// Session name announced by the peer (empty until known).
    pub name: String,
    /// Control-port address of the peer.
    pub addr: SocketAddr,
    /// Connection state.
    pub state: PeerState,
    /// One-way latency from the last clock sync, in microseconds.
    pub latency_us: Option<u64>,
    /// Remote session clock minus local session clock, in microseconds.
    pub clock_offset_us: Option<i64>,
    /// `true` if we invited the peer ([`Session::connect`]).
    pub initiated_by_us: bool,
    /// Packets from this peer detected as lost (RTP sequence gaps).
    pub packets_lost: u64,
    /// MIDI messages synthesised from the peer's recovery journal after a
    /// loss (delivered before the commands of the packet that ended the loss).
    pub recovered: u64,
}

/// Callback for incoming MIDI, see [`Session::set_receiver`].
pub type MidiReceiver = Box<dyn FnMut(&[u8]) + Send>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Port {
    Control,
    Data,
}

enum Event {
    Packet {
        port: Port,
        bytes: Vec<u8>,
        from: SocketAddr,
    },
    Wake,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Initiator: `IN` sent on the control port.
    InviteControl {
        tries: u32,
        next: Instant,
    },
    /// Initiator: control accepted, `IN` sent on the data port.
    InviteData {
        tries: u32,
        next: Instant,
    },
    /// Responder: control accepted, waiting for `IN` on the data port.
    AwaitData,
    Connected,
    Failed {
        retry_at: Option<Instant>,
    },
}

struct Peer {
    name: String,
    ctrl_addr: SocketAddr,
    data_addr: SocketAddr,
    ssrc: Option<u32>,
    token: u32,
    initiated: bool,
    phase: Phase,
    last_rx: Instant,
    next_sync: Instant,
    syncs_sent: u32,
    syncs_unanswered: u32,
    latency: Option<u64>,
    offset: Option<i64>,
    last_seq: Option<u16>,
    lost: u64,
    /// Corrective messages synthesised from recovery journals.
    recovered: u64,
    /// What we know about the peer's MIDI stream (for journal recovery).
    rx: ReceiverState,
    sysex: SysexAssembler,
    feedback_due: Option<Instant>,
    /// Highest of our (extended) sequence numbers acknowledged with `RS`.
    acked: Option<u64>,
    /// Our first (extended) sequence number this peer can have received.
    first_seq: u64,
}

impl Peer {
    fn new(ctrl_addr: SocketAddr, data_addr: SocketAddr, initiated: bool, now: Instant) -> Self {
        Self {
            name: String::new(),
            ctrl_addr,
            data_addr,
            ssrc: None,
            token: 0,
            initiated,
            phase: Phase::AwaitData,
            last_rx: now,
            next_sync: now,
            syncs_sent: 0,
            syncs_unanswered: 0,
            latency: None,
            offset: None,
            last_seq: None,
            lost: 0,
            recovered: 0,
            rx: ReceiverState::new(),
            sysex: SysexAssembler::new(),
            feedback_due: None,
            acked: None,
            first_seq: 0,
        }
    }

    /// Enters `Connected` and resets all per-link state.
    fn link_up(&mut self, now: Instant, next_seq: u64) {
        self.phase = Phase::Connected;
        self.last_rx = now;
        self.next_sync = now;
        self.syncs_sent = 0;
        self.syncs_unanswered = 0;
        self.last_seq = None;
        self.rx = ReceiverState::new();
        self.sysex.reset();
        self.feedback_due = None;
        self.acked = None;
        self.first_seq = next_seq;
    }

    /// Initiator: starts (or restarts) the invitation handshake.
    fn start_invite(&mut self, now: Instant) {
        self.token = random_u32();
        self.ssrc = None;
        self.latency = None;
        self.offset = None;
        self.phase = Phase::InviteControl {
            tries: 0,
            next: now,
        };
    }

    fn has_ssrc(&self, ssrc: u32) -> bool {
        self.ssrc == Some(ssrc)
    }
}

struct State {
    peers: Vec<Peer>,
    /// Extended (non-wrapping) sequence number of the next packet.
    next_seq: u64,
    journal: JournalHistory,
}

struct Shared {
    config: SessionConfig,
    ssrc: u32,
    control: UdpSocket,
    data: UdpSocket,
    local_addr: SocketAddr,
    epoch: Instant,
    closed: AtomicBool,
    state: Mutex<State>,
    receiver: Mutex<Option<MidiReceiver>>,
    events: mpsc::Sender<Event>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

fn random_u32() -> u32 {
    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    hasher.write_u128(nanos);
    let h = hasher.finish();
    (h ^ (h >> 32)) as u32
}

impl Shared {
    /// Session clock in 100 µs units.
    fn clock(&self) -> u64 {
        (self.epoch.elapsed().as_micros() / 100) as u64
    }

    fn send(&self, port: Port, bytes: &[u8], to: SocketAddr) {
        let socket = match port {
            Port::Control => &self.control,
            Port::Data => &self.data,
        };
        if let Err(err) = socket.send_to(bytes, to) {
            debug!(%to, ?port, %err, "rtp-midi send failed");
        }
    }

    fn send_packet(&self, port: Port, packet: &SessionPacket, to: SocketAddr) {
        self.send(port, &applemidi::encode(packet), to);
    }

    fn exchange(&self, token: u32, with_name: bool) -> Exchange {
        Exchange {
            version: PROTOCOL_VERSION,
            token,
            ssrc: self.ssrc,
            name: with_name.then(|| self.config.name.clone()),
        }
    }

    fn send_bye(&self, peer: &Peer) {
        let bye = SessionPacket::Bye(self.exchange(peer.token, false));
        self.send_packet(Port::Control, &bye, peer.ctrl_addr);
    }

    fn after_failure(&self, now: Instant) -> Phase {
        Phase::Failed {
            retry_at: self
                .config
                .auto_reconnect
                .then(|| now + self.config.reconnect_interval),
        }
    }
}

/// A running RTP-MIDI session.
///
/// Created with [`Session::start`]; closed (sending `BY` to all peers) by
/// [`Session::close`] or on drop. All methods take `&self` and may be called
/// from any thread.
pub struct Session {
    shared: Arc<Shared>,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("name", &self.shared.config.name)
            .field("local_addr", &self.shared.local_addr)
            .field("ssrc", &self.shared.ssrc)
            .finish_non_exhaustive()
    }
}

fn bind_pair(ip: IpAddr, port: u16) -> std::io::Result<(UdpSocket, UdpSocket)> {
    let bind_both = |control: u16| -> std::io::Result<(UdpSocket, UdpSocket)> {
        let data = control
            .checked_add(1)
            .ok_or_else(|| std::io::Error::new(ErrorKind::InvalidInput, "port out of range"))?;
        Ok((
            UdpSocket::bind((ip, control))?,
            UdpSocket::bind((ip, data))?,
        ))
    };
    if port != 0 {
        return bind_both(port);
    }
    let mut last_err = std::io::Error::new(ErrorKind::AddrInUse, "no free port pair");
    for _ in 0..64 {
        // Ask the OS for a free port, then try the even/odd pair around it.
        let probe = UdpSocket::bind((ip, 0))?;
        let free = probe.local_addr()?.port();
        drop(probe);
        match bind_both(free & !1) {
            Ok(pair) => return Ok(pair),
            Err(err) => last_err = err,
        }
    }
    Err(last_err)
}

impl Session {
    /// Binds the control port `config.port` and the data port `config.port + 1`
    /// and starts the background threads.
    pub fn start(config: SessionConfig) -> Result<Session, Error> {
        let bind_err = |source| Error::Bind {
            port: config.port,
            source,
        };
        let (control, data) = bind_pair(config.bind_addr, config.port).map_err(bind_err)?;
        let local_addr = control.local_addr().map_err(bind_err)?;
        for socket in [&control, &data] {
            socket.set_read_timeout(Some(TICK * 2)).map_err(bind_err)?;
        }
        let ssrc = config.ssrc.unwrap_or_else(random_u32);
        let (events, event_rx) = mpsc::channel();
        let shared = Arc::new(Shared {
            ssrc,
            control,
            data,
            local_addr,
            epoch: Instant::now(),
            closed: AtomicBool::new(false),
            state: Mutex::new(State {
                peers: Vec::new(),
                // Random 16-bit start; the offset keeps "previous packet" arithmetic positive.
                next_seq: 0x1_0000 + u64::from(random_u32() & 0xFFFF),
                journal: JournalHistory::new(),
            }),
            receiver: Mutex::new(None),
            events: events.clone(),
            config,
        });

        let mut threads = Vec::with_capacity(3);
        let spawned = (|| -> std::io::Result<()> {
            for (port, label) in [(Port::Control, "control"), (Port::Data, "data")] {
                let (sh, tx) = (Arc::clone(&shared), events.clone());
                threads.push(
                    thread::Builder::new()
                        .name(format!("rtpmidi-{label}"))
                        .spawn(move || read_loop(&sh, port, &tx))?,
                );
            }
            let sh = Arc::clone(&shared);
            threads.push(
                thread::Builder::new()
                    .name("rtpmidi-session".to_owned())
                    .spawn(move || worker_loop(&sh, &event_rx))?,
            );
            Ok(())
        })();
        let session = Session {
            shared,
            threads: Mutex::new(threads),
        };
        spawned?; // on error, dropping `session` stops the threads already started
        info!(name = %session.name(), addr = %session.local_addr(), ssrc, "rtp-midi session started");
        Ok(session)
    }

    /// Session name announced to peers.
    pub fn name(&self) -> &str {
        &self.shared.config.name
    }

    /// Local address of the control port (the data port is one higher).
    pub fn local_addr(&self) -> SocketAddr {
        self.shared.local_addr
    }

    /// Our synchronisation source identifier.
    pub fn ssrc(&self) -> u32 {
        self.shared.ssrc
    }

    /// Invites the session whose **control port** is `addr`. Returns
    /// immediately; progress is visible through [`Session::peers`]. Calling it
    /// again for a failed peer restarts the invitation at once.
    pub fn connect(&self, addr: SocketAddr) -> Result<(), Error> {
        let sh = &self.shared;
        if sh.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let data_port = addr.port().checked_add(1).filter(|_| addr.port() != 0);
        let mut data_addr = addr;
        data_addr.set_port(data_port.ok_or(Error::InvalidAddress(addr))?);
        let now = Instant::now();
        {
            let mut st = lock(&sh.state);
            let existing = st
                .peers
                .iter_mut()
                .find(|p| p.initiated && p.ctrl_addr == addr);
            match existing {
                Some(peer) => {
                    if matches!(peer.phase, Phase::Failed { .. }) {
                        peer.start_invite(now);
                    }
                }
                None => {
                    if st.peers.len() >= sh.config.max_peers {
                        return Err(Error::TooManyPeers);
                    }
                    let mut peer = Peer::new(addr, data_addr, true, now);
                    peer.start_invite(now);
                    st.peers.push(peer);
                }
            }
        }
        let _ = sh.events.send(Event::Wake);
        Ok(())
    }

    /// Ends the link(s) with the peer whose control address is `addr`
    /// (sending `BY`) and forgets it. Returns `false` if there was none.
    pub fn disconnect(&self, addr: SocketAddr) -> bool {
        let sh = &self.shared;
        let mut st = lock(&sh.state);
        let before = st.peers.len();
        st.peers.retain(|peer| {
            if peer.ctrl_addr != addr {
                return true;
            }
            if !matches!(peer.phase, Phase::Failed { .. }) {
                sh.send_bye(peer);
            }
            false
        });
        st.peers.len() != before
    }

    /// Sends one or more complete MIDI messages (wire bytes, each starting
    /// with its status byte; SysEx as `F0 … F7`) to all connected peers,
    /// time-stamped with the current session time. Returns the number of
    /// peers the data was sent to (0 if nobody is connected).
    ///
    /// The call encodes and sends on the calling thread (one short mutex,
    /// one `send_to` per peer and packet); it never blocks on the network.
    pub fn send(&self, midi: &[u8]) -> Result<usize, Error> {
        let sh = &self.shared;
        if sh.closed.load(Ordering::Acquire) {
            return Err(Error::Closed);
        }
        let messages = rtp::split_midi(midi).map_err(Error::InvalidMidi)?;
        let chunks = rtp::packetize(&messages, MAX_LIST_BYTES);
        let mut guard = lock(&sh.state);
        let st = &mut *guard;

        // One link per remote SSRC, even if both sides invited each other.
        let mut targets: Vec<(u32, SocketAddr, u64)> = Vec::new();
        for peer in &st.peers {
            if let (Phase::Connected, Some(ssrc)) = (peer.phase, peer.ssrc) {
                if !targets.iter().any(|(s, ..)| *s == ssrc) {
                    let checkpoint = peer.acked.map_or(0, |a| a + 1).max(peer.first_seq);
                    targets.push((ssrc, peer.data_addr, checkpoint));
                }
            }
        }
        if targets.is_empty() {
            return Ok(0);
        }

        for chunk in &chunks {
            let seq = st.next_seq;
            st.next_seq += 1;
            let header = RtpHeader {
                marker: false, // like Apple's implementation
                payload_type: rtp::PAYLOAD_TYPE,
                sequence: seq as u16,
                timestamp: sh.clock() as u32,
                ssrc: sh.ssrc,
            };
            if sh.config.journal {
                let mut journal = Vec::new();
                for (_, addr, checkpoint) in &targets {
                    journal.clear();
                    st.journal.encode(*checkpoint, seq, &mut journal);
                    let packet =
                        rtp::encode_data_packet(&header, false, &chunk.list, Some(&journal))?;
                    sh.send(Port::Data, &packet, *addr);
                }
                for message in messages.get(chunk.messages.clone()).unwrap_or_default() {
                    st.journal.record(seq, message);
                }
            } else {
                let packet = rtp::encode_data_packet(&header, false, &chunk.list, None)?;
                for (_, addr, _) in &targets {
                    sh.send(Port::Data, &packet, *addr);
                }
            }
        }
        Ok(targets.len())
    }

    /// Snapshot of all peers (initiated and accepted).
    pub fn peers(&self) -> Vec<PeerInfo> {
        lock(&self.shared.state)
            .peers
            .iter()
            .map(|peer| PeerInfo {
                name: peer.name.clone(),
                addr: peer.ctrl_addr,
                state: match peer.phase {
                    Phase::Connected => PeerState::Connected,
                    Phase::Failed { .. } => PeerState::Failed,
                    _ => PeerState::Connecting,
                },
                latency_us: peer.latency.map(|units| units * 100),
                clock_offset_us: peer.offset.map(|units| units.saturating_mul(100)),
                initiated_by_us: peer.initiated,
                packets_lost: peer.lost,
                recovered: peer.recovered,
            })
            .collect()
    }

    /// Installs the callback for incoming MIDI. It is called on the session
    /// thread with one complete MIDI message per call (SysEx reassembled).
    /// The callback may call [`Session::send`], but must not call
    /// `set_receiver` and should return quickly.
    pub fn set_receiver(&self, receiver: MidiReceiver) {
        *lock(&self.shared.receiver) = Some(receiver);
    }

    /// Sends `BY` to all peers and stops the background threads. Idempotent;
    /// also performed on drop. The UDP ports are released when the `Session`
    /// is dropped.
    pub fn close(&self) {
        let sh = &self.shared;
        if !sh.closed.swap(true, Ordering::AcqRel) {
            let mut st = lock(&sh.state);
            for peer in &st.peers {
                if !matches!(peer.phase, Phase::Failed { .. }) {
                    sh.send_bye(peer);
                }
            }
            st.peers.clear();
            drop(st);
            info!(name = %self.name(), "rtp-midi session closed");
        }
        let _ = sh.events.send(Event::Wake);
        let threads = std::mem::take(&mut *lock(&self.threads));
        let me = thread::current().id();
        for handle in threads {
            // `close` from inside the receiver callback must not join itself.
            if handle.thread().id() != me {
                let _ = handle.join();
            }
        }
        // Release whatever the callback captured. `try_lock` because `close` may
        // be called from inside the callback, which holds this lock.
        if let Ok(mut receiver) = sh.receiver.try_lock() {
            *receiver = None;
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

fn read_loop(sh: &Shared, port: Port, events: &mpsc::Sender<Event>) {
    let socket = match port {
        Port::Control => &sh.control,
        Port::Data => &sh.data,
    };
    let mut buf = vec![0u8; 65_536];
    while !sh.closed.load(Ordering::Acquire) {
        match socket.recv_from(&mut buf) {
            Ok((len, from)) => {
                let bytes = buf.get(..len).unwrap_or_default().to_vec();
                if events.send(Event::Packet { port, bytes, from }).is_err() {
                    break;
                }
            }
            Err(err) => match err.kind() {
                // Read timeout (WouldBlock on Unix, TimedOut on Windows), or
                // Windows reporting an ICMP "port unreachable" for an earlier send.
                ErrorKind::WouldBlock
                | ErrorKind::TimedOut
                | ErrorKind::Interrupted
                | ErrorKind::ConnectionReset
                | ErrorKind::ConnectionRefused => {}
                _ => {
                    debug!(?port, %err, "rtp-midi receive failed");
                    thread::sleep(TICK);
                }
            },
        }
    }
}

fn worker_loop(sh: &Shared, events: &mpsc::Receiver<Event>) {
    let mut incoming: Vec<Vec<u8>> = Vec::new();
    loop {
        let event = match events.recv_timeout(TICK) {
            Ok(event) => Some(event),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => break,
        };
        if sh.closed.load(Ordering::Acquire) {
            break;
        }
        {
            let mut st = lock(&sh.state);
            let now = Instant::now();
            if let Some(Event::Packet { port, bytes, from }) = event {
                handle_packet(sh, &mut st, port, &bytes, from, now, &mut incoming);
            }
            tick(sh, &mut st, now);
        }
        if !incoming.is_empty() {
            // Called without the state lock so the callback may use the session.
            let mut receiver = lock(&sh.receiver);
            if let Some(callback) = receiver.as_mut() {
                for message in &incoming {
                    callback(message);
                }
            }
            drop(receiver);
            incoming.clear();
        }
    }
}

/// Timers: invitation retries, clock sync, timeouts, receiver feedback.
fn tick(sh: &Shared, st: &mut State, now: Instant) {
    let cfg = &sh.config;
    let mut index = 0;
    while let Some(peer) = st.peers.get_mut(index) {
        index += 1;
        match peer.phase {
            Phase::InviteControl { tries, next } | Phase::InviteData { tries, next }
                if now >= next =>
            {
                if tries >= cfg.invite_max_tries {
                    warn!(addr = %peer.ctrl_addr, "rtp-midi invitation unanswered");
                    peer.phase = sh.after_failure(now);
                    continue;
                }
                let invitation = SessionPacket::Invitation(sh.exchange(peer.token, true));
                let (tries, next) = (tries + 1, now + cfg.invite_retry_interval);
                if matches!(peer.phase, Phase::InviteControl { .. }) {
                    sh.send_packet(Port::Control, &invitation, peer.ctrl_addr);
                    peer.phase = Phase::InviteControl { tries, next };
                } else {
                    sh.send_packet(Port::Data, &invitation, peer.data_addr);
                    peer.phase = Phase::InviteData { tries, next };
                }
            }
            Phase::Failed { retry_at: Some(at) } if now >= at => peer.start_invite(now),
            Phase::Connected if peer.initiated => {
                if now >= peer.next_sync {
                    if peer.syncs_unanswered >= MAX_MISSED_SYNCS {
                        warn!(addr = %peer.ctrl_addr, name = %peer.name, "rtp-midi peer lost (no clock sync)");
                        sh.send_bye(peer);
                        peer.phase = sh.after_failure(now);
                        continue;
                    }
                    let sync = SessionPacket::Sync(Sync {
                        ssrc: sh.ssrc,
                        count: 0,
                        timestamps: [sh.clock(), 0, 0],
                    });
                    sh.send_packet(Port::Data, &sync, peer.data_addr);
                    peer.syncs_unanswered += 1;
                    peer.syncs_sent = peer.syncs_sent.saturating_add(1);
                    peer.next_sync = now
                        + if peer.syncs_sent <= cfg.sync_fast_count {
                            cfg.sync_fast_interval
                        } else {
                            cfg.sync_interval
                        };
                }
            }
            Phase::Connected | Phase::AwaitData => {
                let limit = if peer.phase == Phase::AwaitData {
                    cfg.peer_timeout.min(HANDSHAKE_TIMEOUT)
                } else {
                    cfg.peer_timeout
                };
                if now.saturating_duration_since(peer.last_rx) > limit {
                    warn!(addr = %peer.ctrl_addr, name = %peer.name, "rtp-midi peer timed out");
                    sh.send_bye(peer);
                    index -= 1;
                    st.peers.remove(index);
                    continue;
                }
            }
            _ => {}
        }
        if let (Phase::Connected, Some(due), Some(sequence)) =
            (peer.phase, peer.feedback_due, peer.last_seq)
        {
            if now >= due {
                peer.feedback_due = None;
                let feedback = SessionPacket::ReceiverFeedback {
                    ssrc: sh.ssrc,
                    sequence,
                };
                sh.send_packet(Port::Control, &feedback, peer.ctrl_addr);
            }
        }
    }
}

fn handle_packet(
    sh: &Shared,
    st: &mut State,
    port: Port,
    bytes: &[u8],
    from: SocketAddr,
    now: Instant,
    incoming: &mut Vec<Vec<u8>>,
) {
    if !applemidi::is_session_packet(bytes) {
        if port == Port::Data {
            on_midi(st, bytes, from, now, incoming);
        }
        return;
    }
    let packet = match applemidi::parse(bytes) {
        Ok(packet) => packet,
        Err(err) => {
            trace!(%from, %err, "ignoring malformed AppleMIDI packet");
            return;
        }
    };
    match packet {
        SessionPacket::Invitation(ex) => on_invitation(sh, st, port, &ex, from, now),
        SessionPacket::Accepted(ex) => on_accepted(sh, st, port, ex, now),
        SessionPacket::Rejected(ex) => {
            for peer in &mut st.peers {
                let inviting = matches!(
                    peer.phase,
                    Phase::InviteControl { .. } | Phase::InviteData { .. }
                );
                if peer.initiated && inviting && peer.token == ex.token {
                    warn!(addr = %peer.ctrl_addr, "rtp-midi invitation rejected");
                    peer.phase = sh.after_failure(now);
                }
            }
        }
        SessionPacket::Bye(ex) => on_bye(sh, st, &ex, now),
        SessionPacket::Sync(sync) => on_sync(sh, st, port, &sync, from, now),
        SessionPacket::ReceiverFeedback { ssrc, sequence } => {
            // Extend the 16-bit number to the newest packet <= the last one sent.
            let mut acked = (st.next_seq & !0xFFFF) | u64::from(sequence);
            if acked >= st.next_seq {
                acked = acked.saturating_sub(0x1_0000);
            }
            let mut oldest_needed = st.next_seq;
            for peer in &mut st.peers {
                if peer.phase != Phase::Connected {
                    continue;
                }
                if peer.has_ssrc(ssrc) {
                    peer.last_rx = now;
                }
                if peer.has_ssrc(ssrc) && acked >= peer.first_seq {
                    peer.acked = Some(peer.acked.map_or(acked, |a| a.max(acked)));
                }
                oldest_needed = oldest_needed.min(peer.acked.map_or(peer.first_seq, |a| a + 1));
            }
            st.journal.trim(oldest_needed);
        }
        SessionPacket::BitrateLimit { .. } => {}
    }
}

fn on_invitation(
    sh: &Shared,
    st: &mut State,
    port: Port,
    ex: &Exchange,
    from: SocketAddr,
    now: Instant,
) {
    let reject = || {
        let no = SessionPacket::Rejected(sh.exchange(ex.token, false));
        sh.send_packet(port, &no, from);
    };
    let accept = || {
        let ok = SessionPacket::Accepted(sh.exchange(ex.token, true));
        sh.send_packet(port, &ok, from);
    };
    if ex.version != PROTOCOL_VERSION || ex.ssrc == sh.ssrc {
        reject();
        return;
    }
    let next_seq = st.next_seq;
    match port {
        Port::Control => {
            // A peer that restarted on the same address replaces its old entry.
            st.peers
                .retain(|p| p.initiated || p.ctrl_addr != from || p.has_ssrc(ex.ssrc));
            let known = st
                .peers
                .iter_mut()
                .find(|p| !p.initiated && p.has_ssrc(ex.ssrc) && p.ctrl_addr == from);
            match known {
                Some(peer) => {
                    peer.token = ex.token;
                    peer.last_rx = now;
                }
                None => {
                    if st.peers.len() >= sh.config.max_peers {
                        debug!(%from, "rtp-midi invitation rejected: too many peers");
                        reject();
                        return;
                    }
                    // The data address is fixed when the data invitation arrives.
                    let mut peer = Peer::new(from, from, false, now);
                    peer.ssrc = Some(ex.ssrc);
                    peer.token = ex.token;
                    peer.name = ex.name.clone().unwrap_or_default();
                    st.peers.push(peer);
                }
            }
            accept();
        }
        Port::Data => {
            let known = st
                .peers
                .iter_mut()
                .find(|p| !p.initiated && p.has_ssrc(ex.ssrc) && p.ctrl_addr.ip() == from.ip());
            match known {
                Some(peer) => {
                    peer.data_addr = from;
                    peer.last_rx = now;
                    if peer.phase != Phase::Connected {
                        peer.link_up(now, next_seq);
                        info!(addr = %peer.ctrl_addr, name = %peer.name, "rtp-midi peer connected (accepted)");
                    }
                    accept();
                }
                None => reject(),
            }
        }
    }
}

fn on_accepted(sh: &Shared, st: &mut State, port: Port, ex: Exchange, now: Instant) {
    let next_seq = st.next_seq;
    let Some(peer) = st
        .peers
        .iter_mut()
        .find(|p| p.initiated && p.token == ex.token)
    else {
        return;
    };
    match (port, peer.phase) {
        (Port::Control, Phase::InviteControl { .. }) => {
            peer.ssrc = Some(ex.ssrc);
            peer.name = ex.name.unwrap_or_default();
            peer.phase = Phase::InviteData {
                tries: 1,
                next: now + sh.config.invite_retry_interval,
            };
            let invitation = SessionPacket::Invitation(sh.exchange(peer.token, true));
            sh.send_packet(Port::Data, &invitation, peer.data_addr);
        }
        (Port::Data, Phase::InviteData { .. }) if peer.has_ssrc(ex.ssrc) => {
            peer.link_up(now, next_seq);
            info!(addr = %peer.ctrl_addr, name = %peer.name, "rtp-midi peer connected (initiated)");
        }
        _ => {} // duplicate or stale answer
    }
}

fn on_bye(sh: &Shared, st: &mut State, ex: &Exchange, now: Instant) {
    st.peers.retain_mut(|peer| {
        let matches = if peer.initiated {
            // Before the peer's SSRC is known only the token identifies the link.
            !matches!(peer.phase, Phase::Failed { .. })
                && (peer.has_ssrc(ex.ssrc) || (peer.ssrc.is_none() && peer.token == ex.token))
        } else {
            peer.has_ssrc(ex.ssrc)
        };
        if !matches {
            return true;
        }
        info!(addr = %peer.ctrl_addr, name = %peer.name, "rtp-midi peer said goodbye");
        if peer.initiated {
            peer.phase = sh.after_failure(now);
        }
        peer.initiated
    });
}

fn on_sync(sh: &Shared, st: &mut State, port: Port, sync: &Sync, from: SocketAddr, now: Instant) {
    let clock = sh.clock();
    let [t1, t2, t3] = sync.timestamps;
    let reply = |count: u8, timestamps: [u64; 3]| {
        let packet = SessionPacket::Sync(Sync {
            ssrc: sh.ssrc,
            count,
            timestamps,
        });
        sh.send_packet(port, &packet, from);
    };
    let linked = |p: &&mut Peer| {
        p.has_ssrc(sync.ssrc) && matches!(p.phase, Phase::Connected | Phase::AwaitData)
    };
    match sync.count {
        0 => {
            let mut known = false;
            for peer in st.peers.iter_mut().filter(linked) {
                peer.last_rx = now;
                known = true;
            }
            if known {
                reply(1, [t1, clock, 0]);
            }
        }
        1 => {
            // Answer to our CK0: t1 is our clock, t2 the peer's, t3 = now.
            if let Some(peer) = st.peers.iter_mut().filter(linked).find(|p| p.initiated) {
                let latency = clock.saturating_sub(t1) / 2;
                peer.latency = Some(latency);
                peer.offset = Some(t2.wrapping_sub(t1.wrapping_add(latency)) as i64);
                peer.syncs_unanswered = 0;
                peer.last_rx = now;
                reply(2, [t1, t2, clock]);
            }
        }
        2 => {
            // End of an exchange started by the peer: t2 is our clock, t3 the peer's.
            if let Some(peer) = st.peers.iter_mut().filter(linked).find(|p| !p.initiated) {
                let latency = clock.saturating_sub(t2) / 2;
                peer.latency = Some(latency);
                peer.offset = Some(t3.wrapping_add(latency).wrapping_sub(clock) as i64);
                peer.last_rx = now;
            }
        }
        _ => {}
    }
}

fn on_midi(
    st: &mut State,
    bytes: &[u8],
    from: SocketAddr,
    now: Instant,
    incoming: &mut Vec<Vec<u8>>,
) {
    let packet = match rtp::parse_data_packet(bytes) {
        Ok(packet) => packet,
        Err(err) => {
            debug!(%from, %err, "ignoring malformed RTP-MIDI packet");
            return;
        }
    };
    let ssrc = packet.header.ssrc;
    let Some(peer) = st
        .peers
        .iter_mut()
        .find(|p| p.phase == Phase::Connected && p.has_ssrc(ssrc))
    else {
        trace!(%from, ssrc, "RTP-MIDI packet from unknown source");
        return;
    };
    peer.last_rx = now;
    let sequence = packet.header.sequence;
    if let Some(last) = peer.last_seq {
        match sequence.wrapping_sub(last) {
            1 => {}
            // Duplicate or reordered packet: playing it late would be wrong.
            0 | 0x8000..=0xFFFF => return,
            gap => {
                // Count the loss, abandon a SysEx in progress and repair the
                // stream from the recovery journal if the packet carries one.
                let lost = gap - 1;
                peer.lost += u64::from(lost);
                peer.sysex.reset();
                let recovered = match packet.journal.map(recovery::parse_journal) {
                    Some(Ok(journal)) => {
                        if !recovery::covers_loss(journal.checkpoint, last) {
                            debug!(%from, "rtp-midi journal does not cover the whole loss");
                        }
                        peer.rx.recover(&journal, lost == 1, incoming)
                    }
                    Some(Err(err)) => {
                        debug!(%from, %err, "ignoring malformed recovery journal");
                        0
                    }
                    None => 0,
                };
                peer.recovered += recovered as u64;
                debug!(%from, lost, recovered, "rtp-midi packet loss detected");
            }
        }
    }
    peer.last_seq = Some(sequence);
    peer.feedback_due.get_or_insert(now + FEEDBACK_DELAY);
    for command in packet.commands {
        if let Some(message) = peer.sysex.push(command) {
            peer.rx.record(&message);
            incoming.push(message);
        }
    }
}
