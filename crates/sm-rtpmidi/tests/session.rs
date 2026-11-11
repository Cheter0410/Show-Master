//! Hardware-free integration tests: everything runs on 127.0.0.1 with
//! OS-assigned ports, so tests can run in parallel and in CI on any OS.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sm_rtpmidi::{PeerInfo, PeerState, Session, SessionConfig};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const LOCALHOST: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);
const DEADLINE: Duration = Duration::from_secs(5);

/// Fast timers so that retry/timeout paths finish in well under a second.
fn config(name: &str) -> SessionConfig {
    SessionConfig {
        name: name.to_owned(),
        port: 0,
        bind_addr: LOCALHOST,
        invite_retry_interval: Duration::from_millis(100),
        invite_max_tries: 3,
        auto_reconnect: false,
        reconnect_interval: Duration::from_millis(200),
        sync_fast_count: 2,
        sync_fast_interval: Duration::from_millis(50),
        sync_interval: Duration::from_millis(150),
        peer_timeout: Duration::from_millis(600),
        ..SessionConfig::default()
    }
}

/// Polls `probe` until it yields a value or the deadline passes.
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

fn wait_connected(session: &Session, what: &str) -> PeerInfo {
    wait_for(what, || {
        session
            .peers()
            .into_iter()
            .find(|p| p.state == PeerState::Connected)
    })
}

fn collect_midi(session: &Session) -> mpsc::Receiver<Vec<u8>> {
    let (tx, rx) = mpsc::channel();
    session.set_receiver(Box::new(move |midi| {
        let _ = tx.send(midi.to_vec());
    }));
    rx
}

fn recv_n(rx: &mpsc::Receiver<Vec<u8>>, n: usize) -> Vec<Vec<u8>> {
    (0..n)
        .map(|i| {
            rx.recv_timeout(DEADLINE)
                .unwrap_or_else(|_| panic!("MIDI message {i} of {n} did not arrive"))
        })
        .collect()
}

fn connected_pair(a: SessionConfig, b: SessionConfig) -> (Session, Session) {
    let a = Session::start(a).unwrap();
    let b = Session::start(b).unwrap();
    a.connect(b.local_addr()).unwrap();
    wait_connected(&a, "A connected");
    wait_connected(&b, "B connected");
    (a, b)
}

#[test]
fn port_zero_picks_an_adjacent_pair() {
    let s = Session::start(config("ports")).unwrap();
    let port = s.local_addr().port();
    assert_ne!(port, 0);
    assert_eq!(port % 2, 0, "control port should be even");
    assert_eq!(s.local_addr().ip(), LOCALHOST);
    assert_eq!(s.name(), "ports");
    // Both ports are really taken.
    assert!(UdpSocket::bind((LOCALHOST, port)).is_err());
    assert!(UdpSocket::bind((LOCALHOST, port + 1)).is_err());
    // A second session on the same explicit port fails cleanly.
    let clash = Session::start(SessionConfig {
        port,
        ..config("clash")
    });
    assert!(matches!(clash, Err(sm_rtpmidi::Error::Bind { .. })));
    // After close the session refuses to send; dropping it frees the ports.
    s.close();
    s.close(); // idempotent
    assert!(matches!(s.send(&[0xF8]), Err(sm_rtpmidi::Error::Closed)));
    assert!(matches!(
        s.connect(SocketAddr::new(LOCALHOST, 9)),
        Err(sm_rtpmidi::Error::Closed)
    ));
    drop(s);
    assert!(UdpSocket::bind((LOCALHOST, port)).is_ok());
    assert!(UdpSocket::bind((LOCALHOST, port + 1)).is_ok());
}

#[test]
fn handshake_names_and_clock_sync() {
    let (a, b) = connected_pair(config("Alpha"), config("Beta"));

    let at_a = wait_connected(&a, "A connected");
    assert_eq!(at_a.name, "Beta");
    assert_eq!(at_a.addr, b.local_addr());
    assert!(at_a.initiated_by_us);
    assert_eq!(at_a.packets_lost, 0);

    let at_b = wait_connected(&b, "B connected");
    assert_eq!(at_b.name, "Alpha");
    assert_eq!(at_b.addr, a.local_addr());
    assert!(!at_b.initiated_by_us);

    // CK0/CK1/CK2 gives both sides a latency; on loopback it is tiny.
    for (session, side) in [(&a, "A"), (&b, "B")] {
        let latency = wait_for(&format!("latency at {side}"), || {
            session.peers().first().and_then(|p| p.latency_us)
        });
        assert!(
            latency < 500_000,
            "{side}: implausible latency {latency} µs"
        );
        let offset = session.peers()[0].clock_offset_us.unwrap();
        // Both sessions started within the same test, so the clocks are close.
        assert!(offset.abs() < 5_000_000, "{side}: offset {offset} µs");
    }
    // The link survives many sync rounds and several peer timeouts' worth of time.
    std::thread::sleep(Duration::from_millis(900));
    assert_eq!(a.peers()[0].state, PeerState::Connected);
    assert_eq!(b.peers()[0].state, PeerState::Connected);
}

fn midi_both_ways(journal: bool) {
    let cfg = |name: &str| SessionConfig {
        journal,
        ..config(name)
    };
    let (a, b) = connected_pair(cfg("A"), cfg("B"));
    let at_a = collect_midi(&a);
    let at_b = collect_midi(&b);

    let note_on: &[u8] = &[0x90, 60, 100];
    let cc: &[u8] = &[0xB3, 7, 127];
    let sysex: &[u8] = &[0xF0, 0x7D, 1, 2, 3, 4, 5, 0xF7];
    let quarter_frame: &[u8] = &[0xF1, 0x2A];
    // MTC full frame 01:02:03:04 @ 25 fps.
    let full_frame: &[u8] = &[0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x21, 0x02, 0x03, 0x04, 0xF7];
    let program: &[u8] = &[0xC1, 9];
    let note_off: &[u8] = &[0x80, 60, 0];

    // Separate packets ...
    for msg in [note_on, cc, sysex, quarter_frame, full_frame] {
        assert_eq!(a.send(msg).unwrap(), 1);
    }
    // ... and several messages in one call / one packet.
    let batch = [program, note_off, quarter_frame, full_frame, cc].concat();
    assert_eq!(a.send(&batch).unwrap(), 1);
    // A SysEx too large for one packet is segmented and reassembled.
    let mut big = vec![0xF0, 0x7D];
    big.extend((0..5000u32).map(|i| (i % 128) as u8));
    big.push(0xF7);
    a.send(&big).unwrap();
    a.send(note_on).unwrap();

    let got = recv_n(&at_b, 12);
    let expect: Vec<&[u8]> = vec![
        note_on,
        cc,
        sysex,
        quarter_frame,
        full_frame,
        program,
        note_off,
        quarter_frame,
        full_frame,
        cc,
        &big,
        note_on,
    ];
    assert_eq!(got, expect);

    // And the other direction (responder → initiator).
    assert_eq!(b.send(&[cc, full_frame, note_off].concat()).unwrap(), 1);
    b.send(quarter_frame).unwrap();
    assert_eq!(recv_n(&at_a, 4), [cc, full_frame, note_off, quarter_frame]);

    // Invalid input is rejected without sending anything.
    assert!(matches!(
        a.send(&[0x90, 60]),
        Err(sm_rtpmidi::Error::InvalidMidi(_))
    ));
    assert_eq!(a.send(&[]).unwrap(), 1);
    assert!(at_b.recv_timeout(Duration::from_millis(100)).is_err());
    assert!(at_a.try_recv().is_err());
    assert_eq!(a.peers()[0].packets_lost, 0);
    assert_eq!(b.peers()[0].packets_lost, 0);
}

#[test]
fn midi_both_ways_without_journal() {
    midi_both_ways(false);
}

#[test]
fn midi_both_ways_with_journal() {
    midi_both_ways(true);
}

#[test]
fn send_without_peers_is_a_no_op() {
    let s = Session::start(config("lonely")).unwrap();
    assert_eq!(s.send(&[0x90, 1, 2]).unwrap(), 0);
    assert!(s.peers().is_empty());
}

#[test]
fn close_sends_bye_and_peer_is_dropped() {
    // Initiator closes: the responder forgets the peer.
    let (a, b) = connected_pair(config("A"), config("B"));
    a.close();
    wait_for("B drops A after BY", || b.peers().is_empty().then_some(()));
    assert!(a.peers().is_empty());
    drop(b);

    // Responder is dropped: the initiator marks the peer failed (no auto-reconnect).
    let (a, b) = connected_pair(config("A"), config("B"));
    drop(b);
    wait_for("A sees BY from B", || {
        (a.peers()[0].state == PeerState::Failed).then_some(())
    });
}

#[test]
fn disconnect_single_peer() {
    let (a, b) = connected_pair(config("A"), config("B"));
    assert!(a.disconnect(b.local_addr()));
    assert!(!a.disconnect(b.local_addr()));
    assert!(a.peers().is_empty());
    wait_for("B drops A", || b.peers().is_empty().then_some(()));
}

#[test]
fn auto_reconnect_after_peer_restart() {
    let a = Session::start(SessionConfig {
        auto_reconnect: true,
        ..config("A")
    })
    .unwrap();
    let b = Session::start(config("B")).unwrap();
    let b_port = b.local_addr().port();
    a.connect(b.local_addr()).unwrap();
    wait_connected(&a, "first connection");
    drop(b);
    wait_for("A notices", || {
        (a.peers()[0].state != PeerState::Connected).then_some(())
    });
    // The peer comes back on the same port under a new name.
    let b = Session::start(SessionConfig {
        port: b_port,
        ..config("B2")
    })
    .unwrap();
    let peer = wait_connected(&a, "reconnection");
    assert_eq!(peer.name, "B2");
    assert_eq!(a.peers().len(), 1);
    wait_connected(&b, "B2 sees A");
}

#[test]
fn unanswered_invitation_fails_and_can_be_retried() {
    // A bound but silent socket: invitations go nowhere.
    let silent = UdpSocket::bind((LOCALHOST, 0)).unwrap();
    let a = Session::start(config("A")).unwrap();
    a.connect(silent.local_addr().unwrap()).unwrap();
    assert_eq!(a.peers()[0].state, PeerState::Connecting);
    assert!(a.peers()[0].initiated_by_us);
    wait_for("invitation to give up", || {
        (a.peers()[0].state == PeerState::Failed).then_some(())
    });
    // 3 tries were made, all with the same token.
    silent
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    let mut buf = [0u8; 64];
    let mut invitations = Vec::new();
    while let Ok((n, _)) = silent.recv_from(&mut buf) {
        invitations.push(buf[..n].to_vec());
    }
    assert_eq!(invitations.len(), 3);
    assert!(invitations.iter().all(|i| i == &invitations[0]));
    assert_eq!(&invitations[0][..8], &[0xFF, 0xFF, b'I', b'N', 0, 0, 0, 2]);
    // connect() again restarts a failed peer instead of adding a second one.
    a.connect(silent.local_addr().unwrap()).unwrap();
    assert_eq!(a.peers().len(), 1);
    assert_eq!(a.peers()[0].state, PeerState::Connecting);
}

// ---------------------------------------------------------------------------
// A hand-written foreign peer: plain UDP sockets and literal bytes, sharing no
// code with the crate's codecs.
// ---------------------------------------------------------------------------

const RAW_SSRC: [u8; 4] = [0x3A, 0x11, 0xD6, 0x0E];
const RAW_TOKEN: [u8; 4] = [0x19, 0x49, 0x5C, 0xD2];
const OUR_SSRC: u32 = 0xCAFE_F00D;
const OUR_SSRC_BYTES: [u8; 4] = [0xCA, 0xFE, 0xF0, 0x0D];

struct RawPeer {
    control: UdpSocket,
    data: UdpSocket,
}

impl RawPeer {
    /// Binds an adjacent control/data port pair like a real implementation.
    fn bind() -> Self {
        for _ in 0..100 {
            let control = UdpSocket::bind((LOCALHOST, 0)).unwrap();
            let port = control.local_addr().unwrap().port();
            if port == u16::MAX {
                continue;
            }
            if let Ok(data) = UdpSocket::bind((LOCALHOST, port + 1)) {
                for s in [&control, &data] {
                    s.set_read_timeout(Some(DEADLINE)).unwrap();
                }
                return Self { control, data };
            }
        }
        panic!("no free port pair");
    }

    fn control_addr(&self) -> SocketAddr {
        self.control.local_addr().unwrap()
    }

    fn recv(socket: &UdpSocket) -> (Vec<u8>, SocketAddr) {
        let mut buf = [0u8; 2048];
        let (n, from) = socket.recv_from(&mut buf).expect("no packet from session");
        (buf[..n].to_vec(), from)
    }

    /// Receives on `socket`, skipping packets that start with `skip`.
    fn recv_skipping(socket: &UdpSocket, skip: &[u8]) -> Vec<u8> {
        loop {
            let (bytes, _) = Self::recv(socket);
            if !bytes.starts_with(skip) {
                return bytes;
            }
        }
    }

    fn exchange(command: &[u8; 2], name: Option<&str>) -> Vec<u8> {
        let mut p = vec![0xFF, 0xFF, command[0], command[1], 0, 0, 0, 2];
        p.extend_from_slice(&RAW_TOKEN);
        p.extend_from_slice(&RAW_SSRC);
        if let Some(name) = name {
            p.extend_from_slice(name.as_bytes());
            p.push(0);
        }
        p
    }

    /// Invites `session` on both ports, checking the answers byte by byte.
    fn invite(&self, session: &Session) -> (SocketAddr, SocketAddr) {
        let ctrl = session.local_addr();
        let data = SocketAddr::new(ctrl.ip(), ctrl.port() + 1);
        let mut ok = vec![0xFF, 0xFF, b'O', b'K', 0, 0, 0, 2];
        ok.extend_from_slice(&RAW_TOKEN);
        ok.extend_from_slice(&OUR_SSRC_BYTES);
        ok.extend_from_slice(session.name().as_bytes());
        ok.push(0);

        let invitation = Self::exchange(b"IN", Some("RawPeer"));
        self.control.send_to(&invitation, ctrl).unwrap();
        let (reply, from) = Self::recv(&self.control);
        assert_eq!(reply, ok, "control-port OK");
        assert_eq!(from, ctrl);
        assert_eq!(session.peers()[0].state, PeerState::Connecting);

        self.data.send_to(&invitation, data).unwrap();
        let (reply, from) = Self::recv(&self.data);
        assert_eq!(reply, ok, "data-port OK");
        assert_eq!(from, data);
        (ctrl, data)
    }
}

fn responder_config() -> SessionConfig {
    SessionConfig {
        ssrc: Some(OUR_SSRC),
        ..config("Show Master")
    }
}

#[test]
fn raw_foreign_initiator_full_conversation() {
    let session = Session::start(responder_config()).unwrap();
    assert_eq!(session.ssrc(), OUR_SSRC);
    let midi_in = collect_midi(&session);
    let raw = RawPeer::bind();
    let (ctrl, data) = raw.invite(&session);

    let peer = wait_connected(&session, "raw peer connected");
    assert_eq!(peer.name, "RawPeer");
    assert_eq!(peer.addr, raw.control_addr());
    assert!(!peer.initiated_by_us);
    assert_eq!(peer.latency_us, None);

    // A retransmitted invitation is answered again without duplicating the peer.
    raw.control
        .send_to(&RawPeer::exchange(b"IN", Some("RawPeer")), ctrl)
        .unwrap();
    assert_eq!(&RawPeer::recv(&raw.control).0[..4], b"\xFF\xFFOK");
    assert_eq!(session.peers().len(), 1);

    // --- clock sync: CK0 → CK1 → CK2 --------------------------------------
    let mut ck0 = vec![0xFF, 0xFF, b'C', b'K'];
    ck0.extend_from_slice(&RAW_SSRC);
    ck0.extend_from_slice(&[0, 0, 0, 0]); // count 0 + padding
    let t1: [u8; 8] = [0, 0, 0, 0, 0x12, 0x34, 0x56, 0x78];
    ck0.extend_from_slice(&t1);
    ck0.extend_from_slice(&[0; 16]);
    raw.data.send_to(&ck0, data).unwrap();
    let (ck1, _) = RawPeer::recv(&raw.data);
    assert_eq!(ck1.len(), 36);
    assert_eq!(&ck1[..4], b"\xFF\xFFCK");
    assert_eq!(&ck1[4..8], &OUR_SSRC_BYTES);
    assert_eq!(&ck1[8..12], &[1, 0, 0, 0]);
    assert_eq!(&ck1[12..20], &t1, "timestamp 1 echoed");
    assert_eq!(&ck1[28..36], &[0; 8]);
    let mut ck2 = ck1.clone();
    ck2[4..8].copy_from_slice(&RAW_SSRC);
    ck2[8] = 2;
    ck2[28..36].copy_from_slice(&[0, 0, 0, 0, 0x12, 0x34, 0x56, 0x80]);
    raw.data.send_to(&ck2, data).unwrap();
    let latency = wait_for("latency from CK2", || session.peers()[0].latency_us);
    assert!(latency < 500_000);

    // --- MIDI in: note on, then running status + delta time + SysEx ---------
    let rtp = |seq: u16, list: &[u8]| {
        let mut p = vec![0x80, 0x61];
        p.extend_from_slice(&seq.to_be_bytes());
        p.extend_from_slice(&[0, 0, 0x10, 0]);
        p.extend_from_slice(&RAW_SSRC);
        p.push(list.len() as u8);
        p.extend_from_slice(list);
        p
    };
    raw.data
        .send_to(&rtp(0xFFFF, &[0x90, 0x3C, 0x64]), data)
        .unwrap();
    raw.data
        .send_to(
            &rtp(
                0x0000,
                &[0x90, 0x40, 0x64, 0x05, 0x41, 0x00, 0x00, 0xF0, 0x7E, 0xF7],
            ),
            data,
        )
        .unwrap();
    assert_eq!(
        recv_n(&midi_in, 4),
        [
            vec![0x90, 0x3C, 0x64],
            vec![0x90, 0x40, 0x64],
            vec![0x90, 0x41, 0x00],
            vec![0xF0, 0x7E, 0xF7]
        ]
    );
    // Receiver feedback for the last sequence number arrives on the control port.
    let mut rs = vec![0xFF, 0xFF, b'R', b'S'];
    rs.extend_from_slice(&OUR_SSRC_BYTES);
    rs.extend_from_slice(&[0x00, 0x00, 0, 0]);
    assert_eq!(RawPeer::recv(&raw.control).0, rs);

    // --- packet loss is detected and counted; stale packets are dropped -----
    raw.data
        .send_to(&rtp(0x0003, &[0xB0, 0x07, 0x01]), data)
        .unwrap();
    raw.data
        .send_to(&rtp(0x0002, &[0xB0, 0x07, 0x02]), data)
        .unwrap();
    assert_eq!(recv_n(&midi_in, 1), [vec![0xB0, 0x07, 0x01]]);
    wait_for("loss counted", || {
        (session.peers()[0].packets_lost == 2).then_some(())
    });
    assert!(midi_in.recv_timeout(Duration::from_millis(100)).is_err());

    // --- MIDI out: exact packet bytes (J=0) --------------------------------
    assert_eq!(session.send(&[0x90, 0x3C, 0x7F]).unwrap(), 1);
    let first = RawPeer::recv_skipping(&raw.data, b"\xFF\xFF");
    assert_eq!(first.len(), 16);
    assert_eq!(&first[..2], &[0x80, 0x61]);
    assert_eq!(&first[8..12], &OUR_SSRC_BYTES);
    assert_eq!(&first[12..], &[0x03, 0x90, 0x3C, 0x7F]);
    session.send(&[0xF1, 0x05, 0xB0, 0x07, 0x40]).unwrap();
    let second = RawPeer::recv_skipping(&raw.data, b"\xFF\xFF");
    assert_eq!(&second[12..], &[0x06, 0xF1, 0x05, 0x00, 0xB0, 0x07, 0x40]);
    let seq = |p: &[u8]| u16::from_be_bytes([p[2], p[3]]);
    let ts = |p: &[u8]| u32::from_be_bytes([p[4], p[5], p[6], p[7]]);
    assert_eq!(seq(&second), seq(&first).wrapping_add(1));
    assert!(ts(&second) >= ts(&first));

    // --- BY ends the session -------------------------------------------------
    raw.control
        .send_to(&RawPeer::exchange(b"BY", None), ctrl)
        .unwrap();
    wait_for("peer removed after BY", || {
        session.peers().is_empty().then_some(())
    });
    assert_eq!(session.send(&[0xF8]).unwrap(), 0);
}

#[test]
fn raw_peer_sees_recovery_journal_when_enabled() {
    let session = Session::start(SessionConfig {
        journal: true,
        ..responder_config()
    })
    .unwrap();
    let raw = RawPeer::bind();
    let (ctrl, _) = raw.invite(&session);
    wait_connected(&session, "raw peer connected");

    // First packet: J=1, empty journal (S=1, A=0), checkpoint = seq - 1.
    session.send(&[0x90, 0x3C, 0x64]).unwrap();
    let p1 = RawPeer::recv_skipping(&raw.data, b"\xFF\xFF");
    let s1 = u16::from_be_bytes([p1[2], p1[3]]);
    let prev = s1.wrapping_sub(1).to_be_bytes();
    assert_eq!(&p1[12..], &[0x43, 0x90, 0x3C, 0x64, 0x80, prev[0], prev[1]]);

    // Second packet: journal carries the note-on of packet 1 (chapter N).
    session.send(&[0xB0, 0x07, 0x7F]).unwrap();
    let p2 = RawPeer::recv_skipping(&raw.data, b"\xFF\xFF");
    let c = s1.to_be_bytes();
    assert_eq!(
        &p2[12..],
        &[
            0x43, 0xB0, 0x07, 0x7F, // J=1 LEN=3, CC
            0x20, c[0], c[1], // S=0 A=1 TOTCHAN=0, checkpoint = packet 1
            0x00, 0x07, 0x08, // channel 0, LENGTH 7, chapter N
            0x81, 0xF0, 0x3C, 0xE4, // B=1 LEN=1, no OFFBITS, note 60 Y=1 vel 100
        ]
    );

    // RS acknowledges packet 2: the next journal is empty again.
    let mut rs = vec![0xFF, 0xFF, b'R', b'S'];
    rs.extend_from_slice(&RAW_SSRC);
    rs.extend_from_slice(&s1.wrapping_add(1).to_be_bytes());
    rs.extend_from_slice(&[0, 0]);
    raw.control.send_to(&rs, ctrl).unwrap();
    wait_for("checkpoint to advance", || {
        session.send(&[0xF8]).unwrap();
        let p = RawPeer::recv_skipping(&raw.data, b"\xFF\xFF");
        // J=1 LEN=1, F8, then a header-only journal (S=1, A=0).
        (p.len() == 17 && p[12..15] == [0x41, 0xF8, 0x80]).then_some(())
    });
}

#[test]
fn vanished_initiator_is_timed_out() {
    let session = Session::start(responder_config()).unwrap();
    let raw = RawPeer::bind();
    raw.invite(&session);
    wait_connected(&session, "raw peer connected");
    let connected_at = Instant::now();
    // The raw peer now goes silent without BY (peer_timeout = 600 ms).
    wait_for("silent peer to be dropped", || {
        session.peers().is_empty().then_some(())
    });
    assert!(connected_at.elapsed() >= Duration::from_millis(500));
    // The session says goodbye to the peer it gave up on.
    let bye = RawPeer::recv_skipping(&raw.control, b"\xFF\xFFRS");
    assert_eq!(&bye[..4], b"\xFF\xFFBY");
    assert_eq!(&bye[12..16], &OUR_SSRC_BYTES);
}

#[test]
fn vanished_responder_is_detected_by_missed_syncs() {
    let session = Session::start(responder_config()).unwrap();
    let raw = RawPeer::bind();
    session.connect(raw.control_addr()).unwrap();

    // Control port first ...
    let (invitation, from) = RawPeer::recv(&raw.control);
    assert_eq!(from, session.local_addr());
    assert_eq!(&invitation[..8], &[0xFF, 0xFF, b'I', b'N', 0, 0, 0, 2]);
    assert_eq!(&invitation[12..16], &OUR_SSRC_BYTES);
    assert_eq!(&invitation[16..], b"Show Master\0");
    let mut ok = vec![0xFF, 0xFF, b'O', b'K', 0, 0, 0, 2];
    ok.extend_from_slice(&invitation[8..12]); // echo the initiator token
    ok.extend_from_slice(&RAW_SSRC);
    ok.extend_from_slice(b"Raw Responder\0");
    raw.control.send_to(&ok, from).unwrap();
    // ... then the same invitation on the data port (= control port + 1).
    let (invitation2, from_data) = RawPeer::recv(&raw.data);
    assert_eq!(invitation2, invitation);
    assert_eq!(from_data.port(), session.local_addr().port() + 1);
    assert_eq!(session.peers()[0].state, PeerState::Connecting);
    raw.data.send_to(&ok, from_data).unwrap();

    let peer = wait_connected(&session, "connected to raw responder");
    assert_eq!(peer.name, "Raw Responder");
    assert!(peer.initiated_by_us);

    // Answer the first CK0 properly, check CK2, then go silent.
    let (ck0, _) = RawPeer::recv(&raw.data);
    assert_eq!(ck0.len(), 36);
    assert_eq!(&ck0[..4], b"\xFF\xFFCK");
    assert_eq!(&ck0[4..12], &[0xCA, 0xFE, 0xF0, 0x0D, 0, 0, 0, 0]);
    let mut ck1 = ck0.clone();
    ck1[4..8].copy_from_slice(&RAW_SSRC);
    ck1[8] = 1;
    ck1[20..28].copy_from_slice(&[0, 0, 0, 0, 0, 0x98, 0x96, 0x80]); // our clock: 10_000_000
    raw.data.send_to(&ck1, from_data).unwrap();
    let ck2 = loop {
        let (p, _) = RawPeer::recv(&raw.data);
        if p[8] == 2 {
            break p;
        }
    };
    assert_eq!(&ck2[12..28], &ck1[12..28], "timestamps 1 and 2 echoed");
    assert!(
        ck2[28..36] >= ck0[12..20],
        "timestamp 3 not before timestamp 1"
    );
    let peer = wait_for("latency", || {
        session.peers().into_iter().find(|p| p.latency_us.is_some())
    });
    // Remote clock (1000 s) is far ahead of the freshly started session clock.
    let offset = peer.clock_offset_us.unwrap();
    assert!((990_000_000..=1_000_000_000).contains(&offset), "{offset}");

    // Three unanswered sync rounds later the peer is lost (no auto-reconnect).
    wait_for("peer to be marked failed", || {
        (session.peers()[0].state == PeerState::Failed).then_some(())
    });
    assert_eq!(session.send(&[0xF8]).unwrap(), 0);
}

#[test]
fn invitations_beyond_max_peers_are_rejected() {
    let session = Session::start(SessionConfig {
        max_peers: 1,
        ..responder_config()
    })
    .unwrap();
    let first = RawPeer::bind();
    first.invite(&session);

    let second = UdpSocket::bind((LOCALHOST, 0)).unwrap();
    second.set_read_timeout(Some(DEADLINE)).unwrap();
    let mut invitation = vec![0xFF, 0xFF, b'I', b'N', 0, 0, 0, 2, 9, 9, 9, 9, 1, 2, 3, 4];
    invitation.extend_from_slice(b"Late\0");
    second.send_to(&invitation, session.local_addr()).unwrap();
    let (reply, _) = RawPeer::recv(&second);
    let mut no = vec![0xFF, 0xFF, b'N', b'O', 0, 0, 0, 2, 9, 9, 9, 9];
    no.extend_from_slice(&OUR_SSRC_BYTES);
    assert_eq!(reply, no);
    assert_eq!(session.peers().len(), 1);
    assert!(matches!(
        session.connect("127.0.0.1:9".parse().unwrap()),
        Err(sm_rtpmidi::Error::TooManyPeers)
    ));

    // Wrong protocol version is rejected as well.
    invitation[7] = 3;
    second.send_to(&invitation, session.local_addr()).unwrap();
    assert_eq!(&RawPeer::recv(&second).0[..4], b"\xFF\xFFNO");

    // Garbage on either port is ignored and does not disturb the session.
    for junk in [
        &[][..],
        &[0xFF],
        &[0xFF, 0xFF, b'Z', b'Z', 1],
        &[0x80, 0x61, 0],
        &[0u8; 40],
    ] {
        second.send_to(junk, session.local_addr()).unwrap();
        let data = SocketAddr::new(LOCALHOST, session.local_addr().port() + 1);
        second.send_to(junk, data).unwrap();
    }
    wait_connected(&session, "first peer still connected");
}
