//! The mDNS settings (`remote.mdns`, `rtp_midi.advertise`) were added later:
//! showfiles written before must load with both switched on.
#![allow(clippy::unwrap_used)]

use sm_core::showfile;

#[test]
fn old_showfile_without_mdns_settings_loads_with_defaults_on() {
    let old = r#"{
      "schema_version": 1, "name": "Old",
      "settings": { "remote": { "enabled": true, "port": 7401, "control_pin": "99" } },
      "rtp_midi": { "enabled": true, "session_name": "Rig", "port": 5006, "connect_to": ["10.0.0.2:5004"], "journal": true }
    }"#;
    let show = showfile::parse(old).unwrap();
    assert!(show.settings.remote.mdns);
    assert_eq!(show.settings.remote.port, 7401);
    assert!(show.rtp_midi.advertise);
    assert_eq!(show.rtp_midi.session_name, "Rig");
    assert!(show.rtp_midi.journal);
}

#[test]
fn mdns_settings_roundtrip() {
    let mut show = showfile::parse(r#"{"schema_version": 1, "name": "New"}"#).unwrap();
    assert!(show.settings.remote.mdns && show.rtp_midi.advertise);
    show.settings.remote.mdns = false;
    show.rtp_midi.advertise = false;
    let json = serde_json::to_string(&show).unwrap();
    assert!(json.contains("\"mdns\":false"));
    assert!(json.contains("\"advertise\":false"));
    let back = showfile::parse(&json).unwrap();
    assert!(!back.settings.remote.mdns);
    assert!(!back.rtp_midi.advertise);
}
