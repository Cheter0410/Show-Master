//! RTP-MIDI handshake prototype ("Phase 0 prototype (c)").
//!
//! * `cargo run -p sm-rtpmidi --example rtp_midi_handshake`
//!   Starts two sessions in this process, connects them, exchanges a few notes
//!   and exits with status 0 if everything arrived.
//! * `cargo run -p sm-rtpmidi --example rtp_midi_handshake -- 192.168.1.20:5004`
//!   Starts a session on port 5004 and invites the given peer (control port).
//! * `cargo run -p sm-rtpmidi --example rtp_midi_handshake -- listen [port]`
//!   Only listens; point macOS "Audio MIDI Setup → MIDI Network Setup", iOS or
//!   rtpMIDI (Windows) at this machine's IP and the port.
//!
//! In the two network modes it prints peer state changes and received MIDI and
//! sends a test note (C4 on/off) every second until interrupted.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sm_rtpmidi::{PeerInfo, PeerState, Session, SessionConfig};
use std::error::Error;
use std::net::{IpAddr, Ipv4Addr, ToSocketAddrs};
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::{Duration, Instant};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        None => demo(),
        Some("listen") => network(None, args.get(1).and_then(|p| p.parse().ok())),
        Some(target) => network(Some(target), None),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn describe(peer: &PeerInfo) -> String {
    format!(
        "{:?} {} \"{}\" ({}, latency {})",
        peer.state,
        peer.addr,
        peer.name,
        if peer.initiated_by_us {
            "invited by us"
        } else {
            "invited us"
        },
        peer.latency_us
            .map_or_else(|| "n/a".to_owned(), |us| format!("{us} µs")),
    )
}

/// Prints every change of the peer list.
fn print_changes(session: &Session, previous: &mut Vec<PeerInfo>) {
    let peers = session.peers();
    // Latency changes with every sync; only report state/name changes.
    let key = |p: &PeerInfo| (p.addr, p.state, p.name.clone());
    if peers.iter().map(key).ne(previous.iter().map(key)) {
        if peers.is_empty() {
            println!("[{}] no peers", session.name());
        }
        for peer in &peers {
            println!("[{}] peer: {}", session.name(), describe(peer));
        }
    }
    *previous = peers;
}

fn network(target: Option<&str>, port: Option<u16>) -> Result<(), Box<dyn Error>> {
    let session = Session::start(SessionConfig {
        port: port.unwrap_or(5004),
        ..SessionConfig::new("Show Master RTP-MIDI test")
    })?;
    println!(
        "session \"{}\" listening on UDP {} (control) and {} (data)",
        session.name(),
        session.local_addr().port(),
        session.local_addr().port() + 1
    );
    session.set_receiver(Box::new(|midi| println!("MIDI in:  {}", hex(midi))));
    if let Some(target) = target {
        let addr = target
            .to_socket_addrs()?
            .next()
            .ok_or("address did not resolve")?;
        println!("inviting {addr} ...");
        session.connect(addr)?;
    }
    let mut previous = Vec::new();
    let mut next_note = Instant::now();
    let mut on = false;
    loop {
        print_changes(&session, &mut previous);
        if Instant::now() >= next_note {
            next_note += Duration::from_millis(500);
            on = !on;
            let message = [0x90, 60, if on { 100 } else { 0 }];
            if session.send(&message)? > 0 {
                println!("MIDI out: {}", hex(&message));
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// Two sessions in one process: handshake, clock sync, MIDI both ways, goodbye.
fn demo() -> Result<(), Box<dyn Error>> {
    let local = |name: &str| SessionConfig {
        port: 0,
        bind_addr: IpAddr::V4(Ipv4Addr::LOCALHOST),
        sync_fast_interval: Duration::from_millis(100),
        ..SessionConfig::new(name)
    };
    let a = Session::start(local("Demo A"))?;
    let b = Session::start(local("Demo B"))?;
    println!("A = {}   B = {}", a.local_addr(), b.local_addr());

    let (tx, at_b) = mpsc::channel();
    b.set_receiver(Box::new(move |midi| {
        println!("[Demo B] MIDI in: {}", hex(midi));
        let _ = tx.send(midi.to_vec());
    }));
    let (tx, at_a) = mpsc::channel();
    a.set_receiver(Box::new(move |midi| {
        println!("[Demo A] MIDI in: {}", hex(midi));
        let _ = tx.send(midi.to_vec());
    }));

    println!("A invites B ...");
    a.connect(b.local_addr())?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let (mut seen_a, mut seen_b) = (Vec::new(), Vec::new());
    let synced = |s: &Session| {
        s.peers()
            .first()
            .is_some_and(|p| p.state == PeerState::Connected && p.latency_us.is_some())
    };
    while !(synced(&a) && synced(&b)) {
        print_changes(&a, &mut seen_a);
        print_changes(&b, &mut seen_b);
        if Instant::now() > deadline {
            return Err("handshake did not complete within 5 s".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    for session in [&a, &b] {
        for peer in session.peers() {
            println!("[{}] synced: {}", session.name(), describe(&peer));
        }
    }

    let from_a: [&[u8]; 3] = [&[0x90, 60, 100], &[0x80, 60, 0], &[0xF1, 0x23]];
    for message in from_a {
        println!("[Demo A] MIDI out: {}", hex(message));
        a.send(message)?;
    }
    let from_b: &[u8] = &[0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x21, 0x02, 0x03, 0x04, 0xF7];
    println!("[Demo B] MIDI out: {}", hex(from_b));
    b.send(from_b)?;

    for expected in from_a {
        let got = at_b.recv_timeout(Duration::from_secs(2))?;
        if got != expected {
            return Err(format!("B received {} instead of {}", hex(&got), hex(expected)).into());
        }
    }
    if at_a.recv_timeout(Duration::from_secs(2))? != from_b {
        return Err("A received a corrupted SysEx".into());
    }

    println!("A closes (BY) ...");
    a.close();
    let deadline = Instant::now() + Duration::from_secs(2);
    while !b.peers().is_empty() {
        if Instant::now() > deadline {
            return Err("B did not drop A after BY".into());
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    print_changes(&b, &mut seen_b);
    println!("handshake demo OK");
    Ok(())
}
