//! Setup tabs "MIDI" (port aliases, RTP-MIDI) and "Timecode" (generator,
//! LTC, MTC, jump policy per cue list).

use super::{form, heading, hint, num_field, row, small_button, text_field, unit, SetupState};
use crate::model::UiState;
use crate::theme;
use egui::RichText;
use serde_json::json;
use sm_core::model::{MidiPortAlias, TcJumpPolicy, TcMode};
use sm_core::timecode::{FrameRate, Timecode};
use std::str::FromStr;

#[path = "setup_inputs.rs"]
mod inputs;

/// `host:port` with a non-empty host and a port 1..=65535.
pub(super) fn valid_peer_addr(addr: &str) -> bool {
    let a = addr.trim();
    let Some((host, port)) = a.rsplit_once(':') else {
        return false;
    };
    !host.is_empty()
        && !host.contains(char::is_whitespace)
        && port.parse::<u16>().is_ok_and(|p| p > 0)
}

pub(super) fn midi_tab(ui: &mut egui::Ui, st: &mut SetupState, s: &UiState) {
    let aliases = &s.show.midi_ports;
    let ports = &s.midi.ports;

    heading(ui, "Port aliases");
    hint(
        ui,
        "Cues use the alias, so the show keeps working when the hardware name changes.",
    );
    let mut new_aliases: Option<Vec<MidiPortAlias>> = None;
    egui::Grid::new("midi_aliases")
        .num_columns(3)
        .striped(true)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            ui.label(RichText::new("Alias").color(theme::MUTED));
            ui.label(RichText::new("Device").color(theme::MUTED));
            ui.label("");
            ui.end_row();
            for (i, p) in aliases.iter().enumerate() {
                ui.name_inputs(format!("Alias {} name", i + 1), |ui| {
                    if let Some(name) =
                        text_field(ui, &mut st.drafts, &format!("alias_{i}"), &p.name, 150.0)
                    {
                        let mut a = aliases.clone();
                        a[i].name = name.trim().to_string();
                        new_aliases = Some(a);
                    }
                });
                let missing = !p.device.is_empty() && !ports.contains(&p.device);
                let mut device = p.device.clone();
                let shown = if p.device.is_empty() {
                    RichText::new("(unassigned)").color(theme::MUTED)
                } else if missing {
                    RichText::new(format!("{} (not connected)", p.device)).color(theme::WARN)
                } else {
                    RichText::new(p.device.as_str())
                };
                ui.name_inputs(format!("Alias {} device", i + 1), |ui| {
                    egui::ComboBox::from_id_salt(("alias_dev", i))
                        .width(260.0)
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut device, String::new(), "(unassigned)");
                            for d in ports {
                                ui.selectable_value(&mut device, d.clone(), d.as_str());
                            }
                            if missing {
                                ui.selectable_value(
                                    &mut device,
                                    p.device.clone(),
                                    format!("{} (not connected)", p.device),
                                );
                            }
                        });
                });
                if device != p.device {
                    let mut a = aliases.clone();
                    a[i].device = device;
                    new_aliases = Some(a);
                }
                if small_button(ui, "Remove")
                    .on_hover_text(format!("Remove alias {}", p.name))
                    .clicked()
                {
                    let mut a = aliases.clone();
                    a.remove(i);
                    new_aliases = Some(a);
                }
                ui.end_row();
            }
        });
    if aliases.is_empty() {
        hint(ui, "No aliases yet.");
    }
    ui.horizontal(|ui| {
        if ui.button("Add alias").clicked() {
            let mut a = aliases.clone();
            a.push(MidiPortAlias {
                name: format!("Port {}", a.len() + 1),
                device: ports.first().cloned().unwrap_or_default(),
            });
            new_aliases = Some(a);
        }
        if ui
            .button("MIDI panic")
            .on_hover_text("Send all-notes-off on every port")
            .clicked()
        {
            st.send(json!({ "cmd": "midi_panic" }));
        }
    });
    if let Some(a) = new_aliases {
        st.update_show(json!({ "midi_ports": a }));
    }
    hint(
        ui,
        &format!(
            "Detected devices: {}",
            if ports.is_empty() {
                "none".to_string()
            } else {
                ports.join(", ")
            }
        ),
    );

    inputs::midi_input_section(ui, st, s);

    // ------------------------------------------------------------------
    let rtp = &s.show.rtp_midi;
    heading(ui, "RTP-MIDI (network MIDI)");
    form(ui, "rtp_form", |ui| {
        row(ui, "RTP-MIDI", |ui| {
            let mut on = rtp.enabled;
            if ui.checkbox(&mut on, "Run an RTP-MIDI session").changed() {
                st.update_show(json!({ "rtp_midi": { "enabled": on } }));
            }
        });
        row(ui, "Session name", |ui| {
            if let Some(v) = text_field(ui, &mut st.drafts, "rtp_name", &rtp.session_name, 200.0) {
                st.update_show(json!({ "rtp_midi": { "session_name": v.trim() } }));
            }
        });
        row(ui, "RTP port", |ui| {
            if let Some(v) = num_field(ui, &mut st.drafts, "rtp_port", rtp.port, 1024, 65534, 70.0)
            {
                st.update_show(json!({ "rtp_midi": { "port": v } }));
            }
            unit(ui, "control port; data uses the next one");
        });
        row(ui, "Journal", |ui| {
            let mut on = rtp.journal;
            if ui.checkbox(&mut on, "Recovery journal").changed() {
                st.update_show(json!({ "rtp_midi": { "journal": on } }));
            }
        });
    });

    heading(ui, "Connect to");
    egui::Grid::new("rtp_connect")
        .num_columns(3)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            for (i, a) in rtp.connect_to.iter().enumerate() {
                ui.monospace(a);
                if ui.button("Connect now").clicked() {
                    st.send_toast(
                        json!({ "cmd": "rtp_connect", "addr": a }),
                        format!("Inviting {a}…"),
                    );
                }
                if small_button(ui, "Remove")
                    .on_hover_text(format!("Remove {a}"))
                    .clicked()
                {
                    let mut list = rtp.connect_to.clone();
                    list.remove(i);
                    st.update_show(json!({ "rtp_midi": { "connect_to": list } }));
                }
                ui.end_row();
            }
        });
    if rtp.connect_to.is_empty() {
        hint(
            ui,
            "No peers to invite. Peers can also connect to this session themselves.",
        );
    }
    ui.horizontal(|ui| {
        let resp = ui.add(
            egui::TextEdit::singleline(&mut st.peer_addr)
                .hint_text("192.168.1.20:5004")
                .desired_width(180.0),
        );
        let addr = st.peer_addr.trim().to_string();
        let ok = valid_peer_addr(&addr) && !rtp.connect_to.contains(&addr);
        let enter = resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if ui.add_enabled(ok, egui::Button::new("Add peer")).clicked() || (enter && ok) {
            let mut list = rtp.connect_to.clone();
            list.push(addr);
            st.update_show(json!({ "rtp_midi": { "connect_to": list } }));
            st.peer_addr.clear();
        }
        if !st.peer_addr.trim().is_empty() && !valid_peer_addr(&st.peer_addr) {
            unit(ui, "expected ip:port");
        }
    });

    super::discovery::rtp_discovered(ui, st, s);
    heading(ui, "Peers");
    let peers = &s.midi.rtp.peers;
    if !s.midi.rtp.enabled {
        hint(ui, "RTP-MIDI is off.");
    } else if peers.is_empty() {
        hint(ui, "No peers.");
    } else {
        egui::Grid::new("rtp_peers")
            .num_columns(4)
            .striped(true)
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                for h in ["Name", "Address", "State", "Latency"] {
                    ui.label(RichText::new(h).color(theme::MUTED));
                }
                ui.end_row();
                for p in peers {
                    ui.label(&p.name);
                    ui.monospace(&p.addr);
                    let color = match p.state.as_str() {
                        "connected" => theme::NEXT,
                        "failed" => theme::ERROR,
                        _ => theme::WARN,
                    };
                    ui.label(RichText::new(&p.state).color(color));
                    ui.label(
                        p.latency_ms
                            .map(|l| format!("{l:.1} ms"))
                            .unwrap_or_default(),
                    );
                    ui.end_row();
                }
            });
    }
}

// ----------------------------------------------------------------------
// Timecode
// ----------------------------------------------------------------------

fn rate_label(r: FrameRate) -> &'static str {
    match r {
        FrameRate::F2997Df => "29.97 drop-frame",
        FrameRate::F2997Ndf => "29.97 non-drop",
        other => other.label(),
    }
}

/// Parse `HH:MM:SS:FF` and check the frames against the rate.
pub(super) fn parse_start(text: &str, rate: FrameRate) -> Result<Timecode, String> {
    let tc = Timecode::from_str(text).map_err(|e| e.to_string())?;
    if u32::from(tc.frames) >= rate.nominal() {
        return Err(format!(
            "frames must be below {} at {} fps",
            rate.nominal(),
            rate.label()
        ));
    }
    Ok(tc)
}

pub(super) fn timecode_tab(ui: &mut egui::Ui, st: &mut SetupState, s: &UiState) {
    let tc = &s.show.timecode;
    inputs::timecode_source_section(ui, st, s);
    heading(ui, "Generator");
    form(ui, "tc_gen", |ui| {
        row(ui, "Frame rate", |ui| {
            let mut rate = tc.rate;
            egui::ComboBox::from_id_salt("tc_rate")
                .width(180.0)
                .selected_text(rate_label(rate))
                .show_ui(ui, |ui| {
                    for r in FrameRate::ALL {
                        ui.selectable_value(&mut rate, r, rate_label(r));
                    }
                });
            if rate != tc.rate {
                st.update_show(json!({ "timecode": { "rate": rate } }));
            }
        });
        row(ui, "Start offset", |ui| {
            let current = tc.start.to_string();
            if let Some(v) = text_field(ui, &mut st.drafts, "tc_start", &current, 110.0) {
                match parse_start(&v, tc.rate) {
                    Ok(t) => st.update_show(json!({ "timecode": { "start": t } })),
                    Err(e) => st.error(format!("Start offset: {e}")),
                }
            }
            unit(ui, "HH:MM:SS:FF");
        });
        row(ui, "Mode", |ui| {
            let mut mode = tc.mode;
            let label = |m: TcMode| match m {
                TcMode::Freerun => "Free run",
                TcMode::Transport => "Follow transport (pause / stop)",
            };
            egui::ComboBox::from_id_salt("tc_mode")
                .width(260.0)
                .selected_text(label(mode))
                .show_ui(ui, |ui| {
                    for m in [TcMode::Freerun, TcMode::Transport] {
                        ui.selectable_value(&mut mode, m, label(m));
                    }
                });
            if mode != tc.mode {
                st.update_show(json!({ "timecode": { "mode": mode } }));
            }
        });
        row(ui, "", |ui| {
            let follow = tc.mode == TcMode::Transport;
            let mut on = follow || tc.pause_with_transport;
            if ui
                .add_enabled(
                    !follow,
                    egui::Checkbox::new(&mut on, "Pause button also pauses the timecode"),
                )
                .on_hover_text("Pause (all) pauses the generator, Resume continues it")
                .on_disabled_hover_text("Follow transport always pauses with the show")
                .changed()
            {
                st.update_show(json!({ "timecode": { "pause_with_transport": on } }));
            }
        });
        row(ui, "Test", |ui| {
            for (label, cmd) in [
                ("Start", "tc_start"),
                ("Pause", "tc_pause"),
                ("Stop", "tc_stop"),
            ] {
                if ui
                    .button(label)
                    .on_hover_text(format!("Timecode generator: {label}"))
                    .clicked()
                {
                    st.send(json!({ "cmd": cmd }));
                }
            }
        });
    });

    heading(ui, "LTC (audio)");
    form(ui, "tc_ltc", |ui| {
        row(ui, "LTC", |ui| {
            let mut on = tc.ltc_enabled;
            if ui
                .checkbox(&mut on, "Send LTC on an audio output")
                .changed()
            {
                st.update_show(json!({ "timecode": { "ltc_enabled": on } }));
            }
        });
        row(ui, "LTC channel", |ui| {
            ui.add_enabled_ui(tc.ltc_enabled, |ui| {
                if let Some(v) = num_field(
                    ui,
                    &mut st.drafts,
                    "ltc_ch",
                    tc.ltc_channel + 1,
                    1,
                    128,
                    60.0,
                ) {
                    st.update_show(json!({ "timecode": { "ltc_channel": v - 1 } }));
                }
            });
            unit(ui, "device channel (1 = first)");
        });
        row(ui, "LTC level", |ui| {
            ui.add_enabled_ui(tc.ltc_enabled, |ui| {
                if let Some(v) = num_field(
                    ui,
                    &mut st.drafts,
                    "ltc_db",
                    tc.ltc_level_db,
                    -60.0,
                    0.0,
                    60.0,
                ) {
                    st.update_show(json!({ "timecode": { "ltc_level_db": v } }));
                }
            });
            unit(ui, "dBFS");
        });
    });

    heading(ui, "MTC (MIDI)");
    form(ui, "tc_mtc", |ui| {
        row(ui, "MTC", |ui| {
            let mut on = tc.mtc_enabled;
            if ui.checkbox(&mut on, "Send MIDI timecode").changed() {
                st.update_show(json!({ "timecode": { "mtc_enabled": on } }));
            }
        });
        row(ui, "MTC port", |ui| {
            let mut port = tc.mtc_port.clone();
            let shown = if port.is_empty() {
                "(none)".to_string()
            } else {
                port.clone()
            };
            ui.add_enabled_ui(tc.mtc_enabled, |ui| {
                egui::ComboBox::from_id_salt("mtc_port")
                    .width(260.0)
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut port, String::new(), "(none)");
                        for a in &s.show.midi_ports {
                            ui.selectable_value(
                                &mut port,
                                a.name.clone(),
                                format!("{} (alias)", a.name),
                            );
                        }
                        for d in &s.midi.ports {
                            if !s.show.midi_ports.iter().any(|a| &a.name == d) {
                                ui.selectable_value(&mut port, d.clone(), format!("{d} (device)"));
                            }
                        }
                    });
            });
            if port != tc.mtc_port {
                st.update_show(json!({ "timecode": { "mtc_port": port } }));
            }
        });
    });

    heading(ui, "When timecode jumps");
    hint(
        ui,
        "What happens to timecode-triggered cues of each cue list when the timecode jumps.",
    );
    form(ui, "tc_jump", |ui| {
        for l in &s.show.cue_lists {
            row(ui, &l.name, |ui| {
                let mut p = l.tc_jump;
                let label = |p: TcJumpPolicy| match p {
                    TcJumpPolicy::Fire => "Fire - trigger skipped cues",
                    TcJumpPolicy::Skip => "Skip - ignore skipped cues",
                    TcJumpPolicy::Chase => "Chase - start cues mid-way",
                };
                egui::ComboBox::from_id_salt(("tc_jump", l.id))
                    .width(260.0)
                    .selected_text(label(p))
                    .show_ui(ui, |ui| {
                        for x in [TcJumpPolicy::Fire, TcJumpPolicy::Skip, TcJumpPolicy::Chase] {
                            ui.selectable_value(&mut p, x, label(x));
                        }
                    });
                if p != l.tc_jump {
                    st.send(
                        json!({ "cmd": "update_list", "list": l.id, "patch": { "tc_jump": p } }),
                    );
                }
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_addresses() {
        assert!(valid_peer_addr("192.168.1.20:5004"));
        assert!(valid_peer_addr(" host.local:5004 "));
        assert!(!valid_peer_addr("192.168.1.20"));
        assert!(!valid_peer_addr(":5004"));
        assert!(!valid_peer_addr("a:0"));
        assert!(!valid_peer_addr("a:99999"));
    }

    #[test]
    fn start_offsets() {
        assert!(parse_start("01:00:00:00", FrameRate::F25).is_ok());
        assert!(parse_start("01:00:00:24", FrameRate::F25).is_ok());
        assert!(parse_start("01:00:00:25", FrameRate::F25).is_err());
        assert!(parse_start("01:00:00:29", FrameRate::F30).is_ok());
        assert!(parse_start("1:00", FrameRate::F25).is_err());
    }
}
