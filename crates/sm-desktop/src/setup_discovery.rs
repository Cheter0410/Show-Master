//! Setup > MIDI, RTP-MIDI: sessions found on the local network via
//! mDNS/Bonjour (`midi.rtp.discovered` in the state document), with a
//! "Connect" button that invites them (`rtp_connect`).

use super::{form, heading, hint, row, small_button, unit, SetupState};
use crate::model::{DiscoveredPeer, UiState};
use crate::theme;
use egui::RichText;
use serde_json::json;

/// State of a discovered session as seen from our peer list.
fn link_state<'a>(s: &'a UiState, d: &DiscoveredPeer) -> Option<&'a str> {
    s.midi
        .rtp
        .peers
        .iter()
        .find(|p| p.addr == d.addr)
        .map(|p| p.state.as_str())
}

/// Bonjour switch, visibility of this session and the list of other sessions.
pub(super) fn rtp_discovered(ui: &mut egui::Ui, st: &mut SetupState, s: &UiState) {
    let rtp = &s.show.rtp_midi;
    heading(ui, "Found on the network (Bonjour)");
    form(ui, "rtp_bonjour", |ui| {
        row(ui, "Bonjour", |ui| {
            let mut on = rtp.advertise;
            if ui
                .checkbox(&mut on, "Advertise this session and list other sessions")
                .on_hover_text("mDNS on the local network only; connecting by IP:port always works")
                .changed()
            {
                st.update_show(json!({ "rtp_midi": { "advertise": on } }));
            }
        });
    });
    if !s.midi.rtp.enabled {
        hint(ui, "RTP-MIDI is off.");
        return;
    }
    if !rtp.advertise {
        hint(ui, "Bonjour is off: connect by IP:port above.");
        return;
    }
    let mdns = s.remote.mdns.as_ref();
    match mdns {
        Some(m) if m.rtp_advertised => hint(
            ui,
            &format!(
                "Visible as \"{}\" in Audio MIDI Setup (Mac), on iPad and in rtpMIDI (Windows).",
                m.rtp_instance.as_deref().unwrap_or(&s.midi.rtp.name)
            ),
        ),
        Some(m) => {
            if let Some(e) = &m.error {
                ui.label(RichText::new(format!("Bonjour unavailable: {e}")).color(theme::WARN));
            }
        }
        None => hint(ui, "Bonjour is not running in this process."),
    }

    let found = &s.midi.rtp.discovered;
    if found.is_empty() {
        hint(ui, "No other RTP-MIDI sessions found yet.");
        return;
    }
    let mut keep: Option<String> = None;
    egui::Grid::new("rtp_discovered")
        .num_columns(4)
        .striped(true)
        .spacing([18.0, 6.0])
        .show(ui, |ui| {
            for h in ["Session", "Address", "Host", ""] {
                ui.label(RichText::new(h).color(theme::MUTED));
            }
            ui.end_row();
            for d in found {
                ui.label(&d.name);
                ui.monospace(&d.addr);
                ui.label(RichText::new(&d.host).color(theme::MUTED));
                ui.horizontal(|ui| {
                    match link_state(s, d) {
                        Some("connected") => {
                            ui.label(RichText::new("connected").color(theme::NEXT));
                        }
                        state => {
                            if ui
                                .button("Connect")
                                .on_hover_text(format!("Invite {} ({})", d.name, d.addr))
                                .clicked()
                            {
                                st.send_toast(
                                    json!({ "cmd": "rtp_connect", "addr": d.addr }),
                                    format!("Inviting {}...", d.name),
                                );
                            }
                            if let Some(state) = state {
                                unit(ui, state);
                            }
                        }
                    }
                    if !rtp.connect_to.contains(&d.addr)
                        && small_button(ui, "Keep")
                            .on_hover_text(format!(
                                "Add {} to \"Connect to\" (invited at every start)",
                                d.addr
                            ))
                            .clicked()
                    {
                        keep = Some(d.addr.clone());
                    }
                });
                ui.end_row();
            }
        });
    if let Some(addr) = keep {
        let mut list = rtp.connect_to.clone();
        list.push(addr);
        st.update_show(json!({ "rtp_midi": { "connect_to": list } }));
    }
}
