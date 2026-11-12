//! Advertise and browse through real multicast on this machine (two daemons in
//! one process on a private port, so the system's mDNS responder on 5353 is
//! neither needed nor disturbed).
//!
//! Needs a network interface with multicast (loopback or LAN). Where this
//! machine cannot join the mDNS group at all (some sandboxes / containers) the
//! test reports that and passes without checking anything. A firewall or VPN
//! can still drop the packets, so it is not part of the normal test run (nor
//! of a release build): run it with `-- --ignored`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use sm_mdns::{Desired, Mdns, RtpAdvert};
use std::time::{Duration, Instant};

fn free_port() -> u16 {
    std::net::UdpSocket::bind(("127.0.0.1", 0))
        .and_then(|s| s.local_addr())
        .map(|a| a.port())
        .unwrap()
}

#[test]
#[ignore = "depends on this machine's network (firewall / VPN may drop the multicast); run with: cargo test -p sm-mdns --test loopback -- --ignored"]
fn advertised_session_is_discovered() {
    if let Some(problem) = sm_mdns::multicast_problem() {
        eprintln!("skipped: {problem}");
        return;
    }
    let port = free_port();
    let mut advertiser = Mdns::with_options(port, "sm-test-adv", "advertiser");
    let mut browser = Mdns::with_options(port, "sm-test-browse", "browser");
    advertiser.apply(&Desired {
        http_port: Some(7400),
        rtp: Some(RtpAdvert {
            name: "Loopback Test Session".into(),
            port: 5004,
        }),
        browse_rtp: true,
        ..Default::default()
    });
    browser.apply(&Desired {
        browse_rtp: true,
        ..Desired::default()
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let found = loop {
        advertiser.poll();
        browser.poll();
        let status = browser.status();
        if let Some(d) = status
            .discovered
            .iter()
            .find(|d| d.name == "Loopback Test Session")
        {
            break d.clone();
        }
        assert!(
            Instant::now() < deadline,
            "session not discovered: {status:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(found.addr.ends_with(":5004"), "{found:?}");
    assert_eq!(found.host, "sm-test-adv.local");
    // The advertiser does not list its own session.
    assert!(advertiser
        .status()
        .discovered
        .iter()
        .all(|d| d.name != "Loopback Test Session"));
    assert_eq!(
        advertiser.status().url.as_deref(),
        Some("http://sm-test-adv.local:7400")
    );

    // Withdrawing the advertisement removes it from the browser's list.
    advertiser.apply(&Desired {
        browse_rtp: true,
        ..Desired::default()
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    while browser
        .status()
        .discovered
        .iter()
        .any(|d| d.name == "Loopback Test Session")
    {
        assert!(Instant::now() < deadline, "goodbye not seen");
        browser.poll();
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "depends on this machine's network; run with: cargo test -p sm-mdns --test loopback -- --ignored"]
fn advertised_lantern_is_discovered() {
    if let Some(problem) = sm_mdns::multicast_problem() {
        eprintln!("skipped: {problem}");
        return;
    }
    let port = free_port();
    let mut lantern = Mdns::with_options(port, "lantern-test", "stage-pc");
    let mut browser = Mdns::with_options(port, "sm-test-browse2", "browser");
    lantern.apply(&Desired {
        lantern: Some(RtpAdvert {
            name: "Stage PC".into(),
            port: 53100,
        }),
        ..Desired::default()
    });
    browser.apply(&Desired {
        browse_lantern: true,
        ..Desired::default()
    });
    let deadline = Instant::now() + Duration::from_secs(10);
    let found = loop {
        lantern.poll();
        browser.poll();
        let status = browser.status();
        if let Some(d) = status.lanterns.iter().find(|d| d.name == "Stage PC") {
            break d.clone();
        }
        assert!(
            Instant::now() < deadline,
            "Lantern not discovered: {status:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert!(found.addr.ends_with(":53100"), "{found:?}");
    // Not mixed into the RTP-MIDI list.
    assert!(browser.status().discovered.is_empty());
}
