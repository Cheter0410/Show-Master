//! Announces a (fake) RTP-MIDI session via mDNS for a while, e.g. to check
//! that it appears in Show Master's Setup > MIDI list or in macOS Audio MIDI
//! Setup: `cargo run -p sm-mdns --example announce -- "Test Session" 5004 60`.

fn main() {
    let mut args = std::env::args().skip(1);
    let name = args.next().unwrap_or_else(|| "Test Session".into());
    let port = args.next().and_then(|p| p.parse().ok()).unwrap_or(5004);
    let secs: u64 = args.next().and_then(|s| s.parse().ok()).unwrap_or(60);
    let mut mdns = sm_mdns::Mdns::with_options(sm_mdns::MDNS_PORT, "sm-announce", "announce");
    mdns.apply(&sm_mdns::Desired {
        rtp: Some(sm_mdns::RtpAdvert { name, port }),
        ..sm_mdns::Desired::default()
    });
    for _ in 0..secs * 4 {
        mdns.poll();
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    println!("{:?}", mdns.status());
    mdns.shutdown();
    std::thread::sleep(std::time::Duration::from_millis(300));
}
