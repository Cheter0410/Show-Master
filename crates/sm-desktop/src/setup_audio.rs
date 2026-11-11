//! Setup tabs "Audio" (device, rate, buffer, channels) and "Outputs / Patch"
//! (named outputs, groups, layout templates).

use super::{banner, form, heading, hint, row, small_button, text_field, unit, SetupState};
use crate::model::{DeviceEntry, UiState};
use crate::theme;
use egui::RichText;
use serde_json::json;
use sm_core::model::{AudioSettings, OutputDef, OutputGroup, OutputPatch};

const BUFFERS: [u32; 6] = [64, 128, 256, 512, 1024, 2048];
const RATES: [u32; 4] = [44_100, 48_000, 88_200, 96_000];

/// Audio settings chosen in the window but not applied yet.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Draft {
    backend: Option<String>,
    device: Option<String>,
    sample_rate: u32,
    buffer_frames: Option<u32>,
    channels: u16,
}

impl Draft {
    fn from_settings(a: &AudioSettings) -> Self {
        Self {
            backend: a.backend.clone(),
            device: a.device.clone(),
            sample_rate: a.sample_rate,
            buffer_frames: a.buffer_frames,
            channels: a.channels,
        }
    }

    fn device<'a>(&self, devices: &'a [DeviceEntry]) -> Option<&'a DeviceEntry> {
        let id = self.device.as_deref()?;
        devices
            .iter()
            .find(|d| d.id == id && self.backend.as_deref().is_none_or(|b| b == d.backend))
    }
}

pub(super) fn audio_tab(ui: &mut egui::Ui, st: &mut SetupState, s: &UiState) {
    let settings = &s.show.settings.audio;
    let saved = Draft::from_settings(settings);
    let mut d = st.audio.clone().unwrap_or_else(|| saved.clone());
    let devices = &s.audio.devices;

    // Engine status.
    let a = &s.audio;
    if let Some(err) = &a.error {
        banner(ui, theme::ERROR, &format!("Audio error: {err}"));
    } else if a.is_null {
        banner(ui, theme::WARN, "No audio device - running silent");
    } else {
        let buffer = a
            .buffer_frames
            .map(|b| format!(" · {b} frames"))
            .unwrap_or_default();
        banner(
            ui,
            theme::NEXT,
            &format!(
                "Running: {} · {} · {} Hz{buffer} · {} outputs",
                a.device, a.backend, a.sample_rate, a.channels
            ),
        );
    }

    heading(ui, "Device");
    let default_selected = d.device.is_none() && d.backend.is_none();
    if ui
        .add(
            egui::Button::selectable(default_selected, "System default")
                .min_size(egui::vec2(ui.available_width(), 26.0)),
        )
        .clicked()
    {
        d.backend = None;
        d.device = None;
    }
    let mut backends: Vec<&str> = Vec::new();
    for dev in devices {
        if !backends.contains(&dev.backend.as_str()) {
            backends.push(&dev.backend);
        }
    }
    if devices.is_empty() {
        hint(ui, "No audio devices found.");
    }
    for backend in backends {
        ui.add_space(4.0);
        ui.label(RichText::new(backend).small().strong().color(theme::MUTED));
        for dev in devices.iter().filter(|x| x.backend == backend) {
            let selected = d.device.as_deref() == Some(dev.id.as_str())
                && d.backend.as_deref().is_none_or(|b| b == dev.backend);
            let in_use = !a.is_null && a.device == dev.name && a.backend == dev.backend;
            let mut text = format!("{}   ·   {} out", dev.name, dev.max_output_channels);
            if dev.is_default {
                text.push_str("   ·   default");
            }
            if in_use {
                text.push_str("   ·   in use");
            }
            let mut rt = RichText::new(text);
            if in_use {
                rt = rt.color(theme::NEXT);
            }
            if ui
                .add(
                    egui::Button::selectable(selected, rt)
                        .min_size(egui::vec2(ui.available_width(), 26.0)),
                )
                .clicked()
            {
                d.backend = Some(dev.backend.clone());
                d.device = Some(dev.id.clone());
                if dev.max_output_channels > 0 {
                    d.channels = d.channels.min(dev.max_output_channels as u16).max(1);
                }
                if !dev.sample_rates.is_empty() && !dev.sample_rates.contains(&d.sample_rate) {
                    d.sample_rate = if dev.sample_rates.contains(&48_000) {
                        48_000
                    } else {
                        dev.sample_rates[0]
                    };
                }
            }
        }
    }

    let effective = d
        .device(devices)
        .or_else(|| devices.iter().find(|x| x.is_default));
    let mut rates: Vec<u32> = effective
        .filter(|x| !x.sample_rates.is_empty())
        .map(|x| x.sample_rates.clone())
        .unwrap_or_else(|| RATES.to_vec());
    if !rates.contains(&d.sample_rate) {
        rates.push(d.sample_rate);
    }
    rates.sort_unstable();
    let max_ch = effective
        .map(|x| x.max_output_channels)
        .filter(|n| *n > 0)
        .unwrap_or(64)
        .min(128) as u16;

    heading(ui, "Format");
    form(ui, "audio_form", |ui| {
        row(ui, "Sample rate", |ui| {
            egui::ComboBox::from_id_salt("audio_rate")
                .width(140.0)
                .selected_text(format!("{} Hz", d.sample_rate))
                .show_ui(ui, |ui| {
                    for r in &rates {
                        ui.selectable_value(&mut d.sample_rate, *r, format!("{r} Hz"));
                    }
                });
        });
        row(ui, "Buffer size", |ui| {
            let label = |b: Option<u32>| match b {
                Some(b) => format!(
                    "{b} frames ({:.1} ms)",
                    f64::from(b) * 1000.0 / f64::from(d.sample_rate.max(1))
                ),
                None => "Backend default".into(),
            };
            let mut sizes: Vec<u32> = BUFFERS.to_vec();
            if let Some(b) = d.buffer_frames.filter(|b| !sizes.contains(b)) {
                sizes.push(b);
                sizes.sort_unstable();
            }
            egui::ComboBox::from_id_salt("audio_buffer")
                .width(190.0)
                .selected_text(label(d.buffer_frames))
                .show_ui(ui, |ui| {
                    for b in sizes {
                        ui.selectable_value(&mut d.buffer_frames, Some(b), label(Some(b)));
                    }
                });
            unit(ui, "smaller = lower latency, higher risk of dropouts");
        });
        row(ui, "Output channels", |ui| {
            ui.add(
                egui::DragValue::new(&mut d.channels)
                    .range(1..=max_ch)
                    .speed(0.1),
            );
            unit(ui, &format!("device offers up to {max_ch}"));
        });
    });

    ui.add_space(10.0);
    let changed = d != saved;
    ui.horizontal(|ui| {
        let apply = egui::Button::new(RichText::new("Apply audio settings").strong());
        if ui.add_enabled(changed, apply).clicked() {
            st.update_settings(json!({ "audio": {
                "backend": d.backend,
                "device": d.device,
                "sample_rate": d.sample_rate,
                "buffer_frames": d.buffer_frames,
                "channels": d.channels,
            }}));
            st.audio = None;
            return;
        }
        if ui
            .add_enabled(changed, egui::Button::new("Revert"))
            .clicked()
        {
            st.audio = None;
            return;
        }
        st.audio = changed.then(|| d.clone());
    });
    hint(
        ui,
        "Applying reopens the audio device: all running cues stop.",
    );
}

// ----------------------------------------------------------------------
// Outputs / Patch
// ----------------------------------------------------------------------

fn send_patch(st: &mut SetupState, patch: &OutputPatch) {
    st.update_show(json!({ "patch": patch }));
}

/// Remove output `i`, keeping group member indices consistent.
pub(super) fn remove_output(patch: &mut OutputPatch, i: usize) {
    if i >= patch.outputs.len() {
        return;
    }
    patch.outputs.remove(i);
    let i = i as u16;
    for g in &mut patch.groups {
        g.outputs.retain(|o| *o != i);
        for o in &mut g.outputs {
            if *o > i {
                *o -= 1;
            }
        }
    }
}

pub(super) fn patch_tab(ui: &mut egui::Ui, st: &mut SetupState, s: &UiState) {
    let patch = &s.show.patch;
    let channels = usize::from(s.show.settings.audio.channels)
        .max(s.audio.channels)
        .max(1);

    heading(ui, "Layout templates");
    ui.horizontal_wrapped(|ui| {
        ui.label("Replace the patch by:");
        for l in &s.layouts {
            if ui
                .add(egui::Button::selectable(
                    st.confirm_layout.as_deref() == Some(l.as_str()),
                    l.as_str(),
                ))
                .on_hover_text(format!("Apply the {l} layout template"))
                .clicked()
            {
                st.confirm_layout = Some(l.clone());
            }
        }
    });
    if let Some(layout) = st.confirm_layout.clone() {
        egui::Frame::new()
            .fill(theme::WARN.gamma_multiply(0.12))
            .stroke(egui::Stroke::new(1.0, theme::WARN))
            .corner_radius(5.0)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(format!(
                    "Replace all outputs and groups by the \"{layout}\" template? Cue routings may need checking afterwards (Undo works)."
                ));
                ui.horizontal(|ui| {
                    if ui.button(RichText::new(format!("Apply layout {layout}")).strong()).clicked() {
                        st.send_toast(
                            json!({ "cmd": "apply_layout", "layout": layout }),
                            format!("Layout {layout} applied"),
                        );
                        st.confirm_layout = None;
                    }
                    if ui.button("Cancel").clicked() {
                        st.confirm_layout = None;
                    }
                });
            });
    }

    heading(ui, "Outputs (speakers)");
    let mut new_patch: Option<OutputPatch> = None;
    egui::Grid::new("patch_outputs")
        .num_columns(4)
        .striped(true)
        .spacing([14.0, 6.0])
        .show(ui, |ui| {
            ui.label(RichText::new("#").color(theme::MUTED));
            ui.label(RichText::new("Name").color(theme::MUTED));
            ui.label(RichText::new("Device channel").color(theme::MUTED));
            ui.label("");
            ui.end_row();
            for (i, o) in patch.outputs.iter().enumerate() {
                let n = i + 1;
                ui.label(n.to_string());
                ui.name_inputs(format!("Output {n} name"), |ui| {
                    if let Some(name) =
                        text_field(ui, &mut st.drafts, &format!("out_name_{i}"), &o.name, 140.0)
                    {
                        let mut p = patch.clone();
                        p.outputs[i].name = name.trim().to_string();
                        new_patch = Some(p);
                    }
                });
                let available = usize::from(o.channel) < channels;
                let shown = if available {
                    format!("Channel {}", o.channel + 1)
                } else {
                    format!("Channel {} (not available)", o.channel + 1)
                };
                let mut ch = o.channel;
                ui.name_inputs(format!("Output {n} channel"), |ui| {
                    egui::ComboBox::from_id_salt(("out_ch", i))
                        .width(190.0)
                        .selected_text(if available {
                            RichText::new(shown)
                        } else {
                            RichText::new(shown).color(theme::WARN)
                        })
                        .show_ui(ui, |ui| {
                            let top = channels.max(usize::from(o.channel) + 1);
                            for c in 0..top as u16 {
                                ui.selectable_value(&mut ch, c, format!("Channel {}", c + 1));
                            }
                        });
                });
                if ch != o.channel {
                    let mut p = patch.clone();
                    p.outputs[i].channel = ch;
                    new_patch = Some(p);
                }
                if small_button(ui, "Remove")
                    .on_hover_text(format!("Remove output {n}"))
                    .clicked()
                {
                    let mut p = patch.clone();
                    remove_output(&mut p, i);
                    new_patch = Some(p);
                }
                ui.end_row();
            }
        });
    if patch.outputs.is_empty() {
        hint(ui, "No outputs - add one or apply a layout template.");
    }
    ui.horizontal(|ui| {
        if ui.button("Add output").clicked() {
            let mut p = patch.clone();
            let used: Vec<u16> = p.outputs.iter().map(|o| o.channel).collect();
            let free = (0..channels as u16)
                .find(|c| !used.contains(c))
                .unwrap_or(0);
            p.outputs.push(OutputDef {
                name: format!("Out {}", p.outputs.len() + 1),
                channel: free,
            });
            new_patch = Some(p);
        }
        hint(
            ui,
            &format!("The device has {channels} output channel(s) open."),
        );
    });
    hint(
        ui,
        "Removing outputs changes what existing cue routings point to - check the routing of audio cues afterwards.",
    );

    heading(ui, "Groups");
    if patch.groups.is_empty() {
        hint(
            ui,
            "No groups. Groups are shortcuts for routing to several speakers at once.",
        );
    }
    for (gi, g) in patch.groups.iter().enumerate() {
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .corner_radius(5.0)
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    ui.name_inputs(format!("Group {} name", gi + 1), |ui| {
                        if let Some(name) = text_field(
                            ui,
                            &mut st.drafts,
                            &format!("group_name_{gi}"),
                            &g.name,
                            160.0,
                        ) {
                            let mut p = patch.clone();
                            p.groups[gi].name = name.trim().to_string();
                            new_patch = Some(p);
                        }
                    });
                    let mut layout = g.layout.clone();
                    ui.name_inputs(format!("Group {} layout", gi + 1), |ui| {
                        egui::ComboBox::from_id_salt(("group_layout", gi))
                            .width(110.0)
                            .selected_text(layout.clone().unwrap_or_else(|| "free".into()))
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut layout, None, "free");
                                for l in &s.layouts {
                                    ui.selectable_value(&mut layout, Some(l.clone()), l.as_str());
                                }
                            });
                    });
                    if layout != g.layout {
                        let mut p = patch.clone();
                        p.groups[gi].layout = layout;
                        new_patch = Some(p);
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if small_button(ui, "Remove")
                            .on_hover_text(format!("Remove group {}", g.name))
                            .clicked()
                        {
                            let mut p = patch.clone();
                            p.groups.remove(gi);
                            new_patch = Some(p);
                        }
                    });
                });
                ui.horizontal_wrapped(|ui| {
                    for (oi, o) in patch.outputs.iter().enumerate() {
                        let idx = oi as u16;
                        let mut on = g.outputs.contains(&idx);
                        if ui
                            .checkbox(&mut on, format!("{} {}", oi + 1, o.name))
                            .changed()
                        {
                            let mut p = patch.clone();
                            let members = &mut p.groups[gi].outputs;
                            if on {
                                members.push(idx);
                                members.sort_unstable();
                                members.dedup();
                            } else {
                                members.retain(|x| *x != idx);
                            }
                            new_patch = Some(p);
                        }
                    }
                    if patch.outputs.is_empty() {
                        hint(ui, "No outputs to choose from.");
                    }
                });
            });
        ui.add_space(4.0);
    }
    if ui.button("Add group").clicked() {
        let mut p = patch.clone();
        p.groups.push(OutputGroup {
            name: format!("Group {}", p.groups.len() + 1),
            layout: None,
            outputs: Vec::new(),
        });
        new_patch = Some(p);
    }
    if let Some(p) = new_patch {
        send_patch(st, &p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removing_an_output_shifts_group_members() {
        let mut p = OutputPatch::from_layout("quad").unwrap_or_default();
        p.groups.push(OutputGroup {
            name: "x".into(),
            layout: None,
            outputs: vec![0, 2, 3],
        });
        remove_output(&mut p, 2);
        assert_eq!(p.outputs.len(), 3);
        assert_eq!(p.groups[0].outputs, vec![0, 1, 2]);
        assert_eq!(p.groups[1].outputs, vec![0, 2]);
    }
}
