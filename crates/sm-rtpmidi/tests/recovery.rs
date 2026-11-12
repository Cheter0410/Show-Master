//! Receiver-side recovery with simulated packet loss: two in-process sessions
//! talk through a small UDP proxy (the "test hook") that drops chosen RTP
//! packets on the data port. Everything runs on 127.0.0.1.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sm_rtpmidi::{PeerInfo, PeerState, Session, SessionConfig};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const DEADLINE: Duration = Duration::from_secs(5);

fn config(name: &str, journal: bool) -> SessionConfig {
    SessionConfig {
        name: name.to_owned(),
        port: 0,
        bind_addr: LOCALHOST,
        journal,
        auto_reconnect: false,
        sync_fast_count: 2,
        sync_fast_interval: Duration::from_millis(50),
        ..SessionConfig::default()
    }
}

fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(value) = probe() {
            return value;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn connected(session: &Session) -> Option<PeerInfo> {
    session
        .peers()
        .into_iter()
        .find(|p| p.state == PeerState::Connected)
}

/// Forwards a session's control and data port to `target`, dropping the next
/// `drop` RTP-MIDI data packets from the client side.
struct LossyProxy {
    control: SocketAddr,
    drop: Arc<AtomicUsize>,
    dropped: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl LossyProxy {
    fn start(target: SocketAddr) -> Self {
        let (control, data) = (0..100)
            .find_map(|_| {
                let c = UdpSocket::bind((LOCALHOST, 0)).ok()?;
                let port = c.local_addr().ok()?.port();
                let d = UdpSocket::bind((LOCALHOST, port.checked_add(1)?)).ok()?;
                Some((c, d))
            })
            .expect("no free port pair");
        let drop = Arc::new(AtomicUsize::new(0));
        let dropped = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let control_addr = control.local_addr().unwrap();
        let mut threads = Vec::new();
        for (socket, offset) in [(control, 0u16), (data, 1)] {
            socket
                .set_read_timeout(Some(Duration::from_millis(20)))
                .unwrap();
            let to = SocketAddr::new(target.ip(), target.port() + offset);
            let (drop, dropped, stop) = (drop.clone(), dropped.clone(), stop.clone());
            threads.push(std::thread::spawn(move || {
                let mut client: Option<SocketAddr> = None;
                let mut buf = [0u8; 4096];
                while !stop.load(Ordering::Acquire) {
                    let Ok((n, from)) = socket.recv_from(&mut buf) else {
                        continue;
                    };
                    let bytes = &buf[..n];
                    if from == to {
                        if let Some(client) = client {
                            let _ = socket.send_to(bytes, client);
                        }
                        continue;
                    }
                    client = Some(from);
                    let is_rtp = offset == 1 && bytes.first().is_some_and(|b| b >> 6 == 2);
                    if is_rtp
                        && drop
                            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |d| d.checked_sub(1))
                            .is_ok()
                    {
                        dropped.fetch_add(1, Ordering::AcqRel);
                        continue;
                    }
                    let _ = socket.send_to(bytes, to);
                }
            }));
        }
        Self {
            control: control_addr,
            drop,
            dropped,
            stop,
            threads,
        }
    }

    fn drop_next(&self, n: usize) {
        self.drop.store(n, Ordering::Release);
    }
}

impl Drop for LossyProxy {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        for t in self.threads.drain(..) {
            let _ = t.join();
        }
    }
}

struct Link {
    sender: Session,
    receiver: Session,
    proxy: LossyProxy,
    midi: mpsc::Receiver<Vec<u8>>,
}

/// `sender` invites `receiver` through the lossy proxy.
fn link(journal: bool) -> Link {
    let receiver = Session::start(config("Receiver", false)).unwrap();
    let (tx, midi) = mpsc::channel();
    receiver.set_receiver(Box::new(move |m| {
        let _ = tx.send(m.to_vec());
    }));
    let proxy = LossyProxy::start(receiver.local_addr());
    let sender = Session::start(config("Sender", journal)).unwrap();
    sender.connect(proxy.control).unwrap();
    wait_for("sender connected", || connected(&sender));
    wait_for("receiver connected", || connected(&receiver));
    Link {
        sender,
        receiver,
        proxy,
        midi,
    }
}

impl Link {
    fn recv(&self, n: usize) -> Vec<Vec<u8>> {
        (0..n)
            .map(|i| {
                self.midi
                    .recv_timeout(DEADLINE)
                    .unwrap_or_else(|_| panic!("message {i} of {n} did not arrive"))
            })
            .collect()
    }

    fn nothing_more(&self) {
        assert_eq!(
            self.midi.recv_timeout(Duration::from_millis(150)).ok(),
            None,
            "unexpected extra message"
        );
    }

    fn stats(&self) -> PeerInfo {
        connected(&self.receiver).expect("receiver lost its peer")
    }

    /// Sends `midi` in a packet that the proxy drops.
    fn lose(&self, midi: &[u8]) {
        let before = self.proxy.dropped.load(Ordering::Acquire);
        self.proxy.drop_next(1);
        assert_eq!(self.sender.send(midi).unwrap(), 1);
        wait_for("packet dropped", || {
            (self.proxy.dropped.load(Ordering::Acquire) > before).then_some(())
        });
    }
}

#[test]
fn single_lost_packet_is_recovered_from_the_journal() {
    let l = link(true);
    l.sender.send(&[0x90, 60, 100, 0xB0, 7, 20]).unwrap();
    assert_eq!(l.recv(2), [vec![0x90, 60, 100], vec![0xB0, 7, 20]]);

    // Lost: note off 60, CC7 = 90, program 5.
    l.lose(&[0x80, 60, 0, 0xB0, 7, 90, 0xC0, 5]);
    // The next packet ends the loss; recovered messages come first.
    l.sender.send(&[0xF8]).unwrap();
    assert_eq!(
        l.recv(4),
        [
            vec![0xC0, 5],
            vec![0xB0, 7, 90],
            vec![0x80, 60, 0],
            vec![0xF8],
        ]
    );
    l.nothing_more();
    let stats = l.stats();
    assert_eq!(stats.packets_lost, 1);
    assert_eq!(stats.recovered, 3);
}

#[test]
fn several_lost_packets_recover_recent_note_on_but_never_a_stale_one() {
    let l = link(true);
    l.sender.send(&[0xB1, 1, 0]).unwrap();
    assert_eq!(l.recv(1), [vec![0xB1, 1, 0]]);

    // Packet 1 (lost): note on 64 on channel 2 - two packets before the end of
    // the loss, so the sender marks it as stale (Y=0).
    l.lose(&[0x92, 64, 80]);
    // Packet 2 (lost): CC1 = 33 and note on 65 (in packet N-1: Y=1).
    l.lose(&[0xB1, 1, 33, 0x92, 65, 70]);
    l.sender.send(&[0xFA]).unwrap();
    assert_eq!(
        l.recv(3),
        [vec![0xB1, 1, 33], vec![0x92, 65, 70], vec![0xFA]]
    );
    l.nothing_more();
    let stats = l.stats();
    assert_eq!(stats.packets_lost, 2);
    assert_eq!(stats.recovered, 2);
}

#[test]
fn without_journal_loss_is_only_counted() {
    let l = link(false);
    l.sender.send(&[0x90, 60, 100]).unwrap();
    assert_eq!(l.recv(1), [vec![0x90, 60, 100]]);
    l.lose(&[0x80, 60, 0, 0xB0, 7, 90, 0xC0, 5]);
    l.sender.send(&[0xF8]).unwrap();
    assert_eq!(l.recv(1), [vec![0xF8]]);
    l.nothing_more();
    let stats = l.stats();
    assert_eq!(stats.packets_lost, 1);
    assert_eq!(stats.recovered, 0);
}
