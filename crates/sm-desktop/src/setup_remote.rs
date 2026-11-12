//! Setup tab "Remote": web remote access, PINs, OSC, URLs with QR codes,
//! connected clients.

use super::{banner, form, heading, hint, num_field, row, text_field, unit, SetupState};
use crate::model::UiState;
use crate::theme;
use egui::{Color32, RichText};
use serde_json::json;

/// Quiet zone around a QR code, in modules (the standard asks for 4).
const QUIET: usize = 4;
const QR_SIZE: f32 = 150.0;

/// Module matrix of a QR code including the white quiet zone (true = dark).
pub(super) fn qr_matrix(data: &str) -> Option<(usize, Vec<bool>)> {
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let w = code.width();
    let n = w + 2 * QUIET;
    let colors = code.to_colors();
    let mut out = vec![false; n * n];
    for y in 0..w {
        for x in 0..w {
            out[(y + QUIET) * n + x + QUIET] = colors[y * w + x] == qrcode::Color::Dark;
        }
    }
    Some((n, out))
}

fn qr_texture(
    ui: &egui::Ui,
    cache: &mut std::collections::HashMap<String, Option<egui::TextureHandle>>,
    url: &str,
) -> Option<egui::TextureHandle> {
    if let Some(t) = cache.get(url) {
        return t.clone();
    }
    // Keep the cache small: URLs change rarely (network or port changes).
    if cache.len() > 16 {
        cache.clear();
    }
    let tex = qr_matrix(url).map(|(n, modules)| {
        let pixels = modules
            .iter()
            .map(|dark| {
                if *dark {
                    Color32::BLACK
                } else {
                    Color32::WHITE
                }
            })
            .collect();
        ui.ctx().load_texture(
            format!("qr {url}"),
            egui::ColorImage::new([n, n], pixels),
            egui::TextureOptions::NEAREST,
        )
    });
    cache.insert(url.to_string(), tex.clone());
    tex
}

fn qr_card(ui: &mut egui::Ui, st: &mut SetupState, url: &str, caption: &str) {
    ui.vertical(|ui| {
        match qr_texture(ui, &mut st.qr, url) {
            Some(tex) => {
                // Whole physical pixels per module keep the code crisp.
                let ppp = ui.ctx().pixels_per_point();
                let n = tex.size()[0].max(1) as f32;
                let size = ((QR_SIZE * ppp / n).floor().max(1.0) * n) / ppp;
                ui.add(
                    egui::Image::new(egui::load::SizedTexture::new(
                        tex.id(),
                        egui::vec2(size, size),
                    ))
                    .alt_text(format!("QR code {url}")),
                );
            }
            None => {
                ui.label("(QR code unavailable)");
            }
        }
        ui.label(RichText::new(caption).strong());
        ui.monospace(url);
    });
}

/// Digits only, at most 12.
pub(super) fn valid_pin(pin: &str) -> bool {
    pin.len() <= 12 && pin.chars().all(|c| c.is_ascii_digit())
}

fn fmt_since(since_ms: u64) -> String {
    use chrono::TimeZone;
    i64::try_from(since_ms)
        .ok()
        .and_then(|ms| chrono::Local.timestamp_millis_opt(ms).single())
        .map(|t| t.format("%H:%M:%S").to_string())
        .unwrap_or_default()
}

fn short_agent(agent: &str) -> &str {
    for (needle, name) in [
        ("iPad", "iPad"),
        ("iPhone", "iPhone"),
        ("Android", "Android"),
        ("Macintosh", "Mac"),
        ("Windows", "Windows"),
        ("Linux", "Linux"),
        ("Companion", "Companion"),
    ] {
        if agent.contains(needle) {
            return name;
        }
    }
    if agent.is_empty() {
        "-"
    } else {
        agent
    }
}

/// What to type on a tablet: `http://showmaster.local` without the scheme
/// (`/` opens the read-only view for network devices, with a "Control" button).
pub(crate) fn short_address(url: &str) -> &str {
    url.trim_start_matches("http://").trim_end_matches('/')
}

/// The `showmaster.local` address (mDNS), shown big above the IP URLs.
fn mdns_url(ui: &mut egui::Ui, s: &UiState) {
    let Some(m) = s.remote.mdns.as_ref().filter(|m| m.enabled) else {
        return;
    };
    match &m.url {
        Some(url) => {
            ui.horizontal_wrapped(|ui| {
                ui.label("On a phone or tablet simply type");
                ui.label(
                    RichText::new(short_address(url))
                        .monospace()
                        .strong()
                        .size(20.0)
                        .color(theme::RUNNING),
                );
            });
            unit(
                ui,
                "by name (Bonjour) - opens the view; its Control button asks for the control PIN. If the name does not work (some Android devices), use the IP address below",
            );
            ui.add_space(4.0);
        }
        None => {
            if let Some(e) = &m.error {
                ui.label(
                    RichText::new(format!(
                        "Bonjour name unavailable ({e}) - use the IP address."
                    ))
                    .color(theme::WARN),
                );
            }
        }
    }
}

pub(super) fn remote_tab(ui: &mut egui::Ui, st: &mut SetupState, s: &UiState) {
    let r = &s.show.settings.remote;
    heading(ui, "Remote access");
    form(ui, "remote_form", |ui| {
        row(ui, "Remote", |ui| {
            let mut on = r.enabled;
            if ui
                .checkbox(&mut on, "Allow devices on the local network")
                .changed()
            {
                st.update_settings(json!({ "remote": { "enabled": on } }));
            }
        });
        row(ui, "Web port", |ui| {
            if let Some(v) = num_field(ui, &mut st.drafts, "remote_port", r.port, 1024, 65534, 70.0)
            {
                st.update_settings(json!({ "remote": { "port": v } }));
            }
            if s.remote.port != 0 && s.remote.port != r.port {
                unit(ui, &format!("currently {}", s.remote.port));
            }
        });
        for (label, key, value, note) in [
            (
                "Control PIN",
                "control_pin",
                &r.control_pin,
                "for /remote, Stream Deck, Companion; empty = no PIN",
            ),
            (
                "View PIN",
                "view_pin",
                &r.view_pin,
                "for /view; empty = no PIN",
            ),
        ] {
            row(ui, label, |ui| {
                if let Some(v) = text_field(ui, &mut st.drafts, key, value, 110.0) {
                    let v = v.trim().to_string();
                    if valid_pin(&v) {
                        st.update_settings(json!({ "remote": { key: v } }));
                    } else {
                        st.error("A PIN may only contain digits (max. 12).");
                    }
                }
                unit(ui, note);
            });
        }
        row(ui, "Bonjour", |ui| {
            let mut on = r.mdns;
            if ui
                .checkbox(&mut on, "Advertise by name (mDNS)")
                .on_hover_text(
                    "Tablets can open the remote by name instead of the IP address. \
                     Local network only (multicast); the IP address always works.",
                )
                .changed()
            {
                st.update_settings(json!({ "remote": { "mdns": on } }));
            }
            if let Some(host) = s
                .remote
                .mdns
                .as_ref()
                .filter(|m| m.active && r.mdns)
                .map(|m| &m.host)
            {
                unit(ui, host);
            }
        });
        row(ui, "OSC port", |ui| {
            if let Some(v) = num_field(ui, &mut st.drafts, "osc_port", r.osc_port, 0, 65534, 70.0) {
                st.update_settings(json!({ "remote": { "osc_port": v } }));
            }
            unit(ui, "0 = off");
        });
    });
    if r.osc_port != 0 {
        banner(
            ui,
            theme::WARN,
            "OSC has no authentication: anyone on the network can send commands. Use it on trusted show networks only.",
        );
    }
    hint(
        ui,
        "Changes of the web port and of remote on/off take effect after Show Master is restarted.",
    );

    heading(ui, "Open on a tablet");
    if !r.enabled {
        hint(ui, "Remote access is disabled.");
    } else if s.remote.urls.is_empty() {
        hint(
            ui,
            "No network address found - is this computer connected to the show network?",
        );
    } else {
        mdns_url(ui, s);
        for u in &s.remote.urls {
            ui.horizontal(|ui| {
                ui.monospace(format!("{u}/remote"));
                unit(ui, "control");
                ui.add_space(12.0);
                ui.monospace(format!("{u}/view"));
                unit(ui, "view only");
            });
        }
        let base = s.remote.urls[0].clone();
        ui.add_space(6.0);
        ui.horizontal_top(|ui| {
            qr_card(ui, st, &format!("{base}/remote"), "Control (PIN)");
            ui.add_space(24.0);
            qr_card(ui, st, &format!("{base}/view"), "View only");
        });
    }

    heading(ui, "Paired devices");
    hint(
        ui,
        "Pair a tablet once: it scans the code and is logged in from then on, without a PIN. \
         Stored on this computer; revoke a device to log it out for good.",
    );
    if let Some((url, name)) = st.pair_shown.clone() {
        ui.horizontal_top(|ui| {
            qr_card(ui, st, &url, &format!("Scan with \"{name}\""));
            ui.vertical(|ui| {
                ui.label("Open the camera on the tablet and scan the code.");
                ui.label(
                    RichText::new("Anyone who scans it gets access - close it when done.")
                        .small()
                        .color(theme::WARN),
                );
                if ui.button("Done").clicked() {
                    st.pair_shown = None;
                }
            });
        });
    } else if r.enabled && !s.remote.urls.is_empty() {
        ui.horizontal(|ui| {
            if ui
                .button("Pair a control device")
                .on_hover_text("GO, Stop, PANIC ... without typing the control PIN")
                .clicked()
            {
                st.outbox_push(super::Out::Pair("control"));
            }
            if ui
                .button("Pair a view-only device")
                .on_hover_text("Display / stage manager view")
                .clicked()
            {
                st.outbox_push(super::Out::Pair("view"));
            }
        });
    }
    if !s.remote.pairings.is_empty() {
        egui::Grid::new("remote_pairings")
            .num_columns(4)
            .striped(true)
            .spacing([18.0, 6.0])
            .show(ui, |ui| {
                for p in &s.remote.pairings {
                    ui.label(RichText::new(&p.name).strong());
                    ui.label(&p.role);
                    ui.label(fmt_since(p.created_ms));
                    if ui
                        .button("Revoke")
                        .on_hover_text(format!("Log {} out for good", p.name))
                        .clicked()
                    {
                        st.send_toast(
                            json!({ "cmd": "revoke_pairing", "id": p.id }),
                            format!("{} revoked", p.name),
                        );
                    }
                    ui.end_row();
                }
            });
        if s.remote.pairings.len() > 1 && ui.button("Revoke all").clicked() {
            st.send_toast(json!({ "cmd": "revoke_all_pairings" }), "All paired devices revoked");
        }
    }

    heading(ui, "Connected clients");
    let clients = &s.remote.clients;
    if clients.is_empty() {
        hint(ui, "No clients.");
        return;
    }
    egui::Grid::new("remote_clients")
        .num_columns(5)
        .striped(true)
        .spacing([18.0, 6.0])
        .show(ui, |ui| {
            for h in ["Role", "Address", "Since", "Device", ""] {
                ui.label(RichText::new(h).color(theme::MUTED));
            }
            ui.end_row();
            for c in clients {
                let color = match c.role.as_str() {
                    "admin" => theme::ERROR,
                    "control" => theme::RUNNING,
                    _ => theme::MUTED,
                };
                ui.label(RichText::new(&c.role).strong().color(color));
                ui.monospace(&c.addr);
                ui.label(fmt_since(c.since_ms));
                if c.device.is_empty() {
                    ui.label(short_agent(&c.agent)).on_hover_text(&c.agent);
                } else {
                    ui.label(&c.device).on_hover_text(&c.agent);
                }
                if ui
                    .button("Disconnect")
                    .on_hover_text(format!("Disconnect {}", c.addr))
                    .clicked()
                {
                    st.send_toast(
                        json!({ "cmd": "kick_client", "id": c.id }),
                        format!("{} disconnected", c.addr),
                    );
                }
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qr_has_quiet_zone_and_finder() {
        let (n, m) = qr_matrix("http://192.168.1.2:7400/remote").unwrap_or_default();
        assert!(n > 2 * QUIET);
        assert_eq!(m.len(), n * n);
        // Quiet zone is white, the finder pattern's corner is dark.
        assert!(!m[0]);
        assert!(m[QUIET * n + QUIET]);
    }

    #[test]
    fn short_addresses() {
        assert_eq!(short_address("http://showmaster.local"), "showmaster.local");
        assert_eq!(
            short_address("http://showmaster.local:7400"),
            "showmaster.local:7400"
        );
    }

    #[test]
    fn pins() {
        assert!(valid_pin(""));
        assert!(valid_pin("1234"));
        assert!(!valid_pin("12a4"));
        assert!(!valid_pin("1234567890123"));
    }
}
