//! mDNS/Bonjour in the Setup window: RTP-MIDI sessions found on the network
//! (published in `midi.rtp.discovered`) are listed with a Connect button that
//! sends `rtp_connect`; Setup > Remote shows the `.local` address. The mDNS
//! status comes from a fake `net_status` hook instead of real multicast.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod common;

use common::settle;
use egui_kittest::kittest::Queryable;
use egui_kittest::Harness;
use serde_json::json;
use sm_core::model::Show;
use sm_desktop::client::Client;
use sm_desktop::App;
use sm_server::runtime::{self, Config, Handle};
use std::net::UdpSocket;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct Env {
    rt: tokio::runtime::Runtime,
    handle: Handle,
    /// Stands in for the discovered session (never answers).
    _peer: (UdpSocket, UdpSocket),
    peer_addr: String,
}

fn peer_pair() -> (UdpSocket, UdpSocket) {
    for _ in 0..100 {
        let c = UdpSocket::bind("127.0.0.1:0").unwrap();
        let port = c.local_addr().unwrap().port();
        if let Some(d) = port
            .checked_add(1)
            .and_then(|p| UdpSocket::bind(("127.0.0.1", p)).ok())
        {
            return (c, d);
        }
    }
    panic!("no free port pair");
}

fn start(advertise: bool) -> Env {
    let mut show = Show::default();
    show.settings.audio.backend = Some("null".into());
    show.rtp_midi.enabled = true;
    show.rtp_midi.port = 0;
    show.rtp_midi.session_name = "Desk".into();
    show.rtp_midi.advertise = advertise;
    let peer = peer_pair();
    let peer_addr = peer.0.local_addr().unwrap().to_string();
    let addr = peer_addr.clone();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    let (handle, _thread) = runtime::spawn(Config {
        show: Some(show),
        midi_backend: Some(Box::new(sm_io::midi::NoMidi)),
        net_status: Some(Arc::new(move || {
            json!({
                "mdns": {
                    "enabled": true, "active": true, "host": "showmaster.local",
                    "url": "http://showmaster.local:7400", "rtp_advertised": true, "error": null,
                },
                "discovered": [
                    { "name": "Studio Mac", "addr": addr, "host": "studio-mac.local" },
                ],
            })
        })),
        ..Config::default()
    });
    Env {
        rt,
        handle,
        _peer: peer,
        peer_addr,
    }
}

impl Env {
    fn harness(&self) -> Harness<'static, App> {
        let client = Client::new(self.handle.clone(), self.rt.handle().clone());
        let app = App::new(&egui::Context::default(), client, None);
        // Tall window: the whole MIDI tab fits without scrolling.
        let mut h = Harness::builder()
            .with_size(egui::vec2(1440.0, 2400.0))
            .build_ui_state(
                |ui, app: &mut App| {
                    app.logic(ui.ctx());
                    app.ui_root(ui);
                },
                app,
            );
        settle(&mut h);
        h
    }

    fn wait(
        &self,
        h: &mut Harness<'static, App>,
        what: &str,
        pred: impl Fn(&serde_json::Value) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !pred(&self.handle.state()) {
            assert!(Instant::now() < deadline, "timed out waiting for: {what}");
            h.step();
            std::thread::sleep(Duration::from_millis(10));
        }
        settle(h);
    }
}

/// Scrolls the widget into view (the Setup body is a scroll area) and clicks it.
fn click(h: &mut Harness<'static, App>, label: &str) {
    h.get_by_label(label).scroll_to_me();
    settle(h);
    h.get_by_label(label).click();
    settle(h);
}

fn open_setup(h: &mut Harness<'static, App>, tab: &str) {
    h.get_by_label("Setup").click();
    settle(h);
    h.get_by_label(tab).click();
    settle(h);
}

#[test]
fn discovered_sessions_are_listed_and_connect_sends_rtp_connect() {
    let env = start(true);
    let mut h = env.harness();
    env.wait(&mut h, "discovered peer published", |s| {
        s["midi"]["rtp"]["discovered"][0]["name"] == "Studio Mac"
    });
    open_setup(&mut h, "MIDI");
    assert!(h.query_by_label("Found on the network (Bonjour)").is_some());
    assert!(h.query_by_label("Studio Mac").is_some());
    assert!(h.query_by_label(&env.peer_addr).is_some());
    assert!(h.query_by_label("studio-mac.local").is_some());
    assert!(h.query_by_label_contains("Visible as \"Desk\"").is_some());

    click(&mut h, "Connect");
    let addr = env.peer_addr.clone();
    env.wait(&mut h, "peer invited", move |s| {
        s["midi"]["rtp"]["peers"]
            .as_array()
            .is_some_and(|p| p.iter().any(|p| p["addr"] == addr.as_str()))
    });
    // The peer never answers: it is listed as connecting (here and under
    // "Peers"), not as connected, and can be invited again.
    assert!(h.query_all_by_label("connecting").count() >= 2);
    assert!(h.query_by_label("connected").is_none());
    assert!(h.query_by_label("Connect").is_some());

    // "Keep" adds it to the persistent invitation list.
    click(&mut h, "Keep");
    let addr = env.peer_addr.clone();
    env.wait(&mut h, "kept in connect_to", move |s| {
        s["show"]["rtp_midi"]["connect_to"][0] == addr.as_str()
    });
}

#[test]
fn bonjour_switch_for_rtp_midi() {
    let env = start(false);
    let mut h = env.harness();
    open_setup(&mut h, "MIDI");
    assert!(h.query_by_label_contains("Bonjour is off").is_some());
    assert!(h.query_by_label("Studio Mac").is_none());
    click(&mut h, "Advertise this session and list other sessions");
    env.wait(&mut h, "advertise on", |s| {
        s["show"]["rtp_midi"]["advertise"] == true
    });
    assert!(h.query_by_label("Studio Mac").is_some());
}

#[test]
fn remote_tab_shows_the_local_name() {
    let env = start(true);
    let mut h = env.harness();
    open_setup(&mut h, "Remote");
    // The .local URL is only listed next to IP URLs (needs a network address).
    let has_ip = env.handle.state()["remote"]["urls"]
        .as_array()
        .is_some_and(|u| !u.is_empty());
    if has_ip {
        // Shown big in the Remote tab and in the status bar.
        assert_eq!(h.query_all_by_label("showmaster.local:7400").count(), 2);
    }
    assert!(h.query_by_label("showmaster.local").is_some());
    click(&mut h, "Advertise by name (mDNS)");
    env.wait(&mut h, "mdns off", |s| {
        s["show"]["settings"]["remote"]["mdns"] == false
    });
}
