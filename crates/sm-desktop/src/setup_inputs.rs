//! Setup: external inputs. MIDI tab: listened ports, activity, MSC receive
//! and the remote-control mapping (with "learn"). Timecode tab: the
//! timecode source (generator or chase of MTC / LTC).

use super::super::{
    form, heading, hint, num_field, row, small_button, text_field, unit, SetupState,
};
use crate::model::{MidiLast, UiState};
use crate::theme;
use egui::{Id, RichText};
use serde_json::json;
use sm_core::midi_in::action_label;
use sm_core::model::{
    MidiInputMode, MidiMapAction, MidiMapping, MidiTriggerKind, Show, TcInputKind, TcSource,
};

/// Mapping row waiting for an incoming message: (index, message count at start).
fn learn_id() -> Id {
    Id::unique("setup_midi_learn")
}

fn learning(ui: &egui::Ui) -> Option<(usize, u64)> {
    ui.ctx().data(|d| d.get_temp(learn_id()))
}

fn set_learning(ui: &egui::Ui, v: Option<(usize, u64)>) {
    ui.ctx().data_mut(|d| match v {
        Some(v) => {
            d.insert_temp(learn_id(), v);
        }
        None => d.remove::<(usize, u64)>(learn_id()),
    });
}

/// What a port is called in the show: the alias that points at it, else its name.
fn port_value(show: &Show, port: &str) -> String {
    show.midi_ports
        .iter()
        .find(|a| a.device == port)
        .map(|a| a.name.clone())
        .unwrap_or_else(|| port.to_string())
}

fn port_label(show: &Show, value: &str) -> String {
    match show.midi_ports.iter().find(|a| a.name == value) {
        Some(a) if a.device != a.name => format!("{} ({})", a.name, a.device),
        _ => value.to_string(),
    }
}

fn mode_label(m: MidiInputMode) -> &'static str {
    match m {
        MidiInputMode::None => "Nothing (off)",
        MidiInputMode::All => "All ports",
        MidiInputMode::Selected => "Selected ports",
    }
}

fn kind_label(k: MidiTriggerKind) -> &'static str {
    match k {
        MidiTriggerKind::NoteOn => "Note on",
        MidiTriggerKind::ControlChange => "Control change",
        MidiTriggerKind::ProgramChange => "Program change",
    }
}

/// Fill a mapping's trigger from a received message; `None` if it cannot trigger.
pub(super) fn learned(show: &Show, m: &MidiMapping, last: &MidiLast) -> Option<MidiMapping> {
    let kind = match last.kind.as_str() {
        "note_on" => MidiTriggerKind::NoteOn,
        "control_change" => MidiTriggerKind::ControlChange,
        "program_change" => MidiTriggerKind::ProgramChange,
        _ => return None,
    };
    Some(MidiMapping {
        port: port_value(show, &last.port),
        kind,
        channel: last.channel,
        number: last.number,
        min_value: if kind == MidiTriggerKind::ControlChange && last.value > 0 {
            m.min_value.clamp(1, last.value)
        } else {
            m.min_value.max(1)
        },
        ..m.clone()
    })
}

pub(super) fn midi_input_section(ui: &mut egui::Ui, st: &mut SetupState, s: &UiState) {
    let show = &s.show;
    let mi = &show.midi_input;
    let inp = &s.inputs;
    heading(ui, "MIDI input");
    hint(
        ui,
        "Incoming MIDI can run the show (mapping below) and answer MIDI Show Control. \
         Nothing reacts until it is set up here.",
    );
    form(ui, "midi_in_form", |ui| {
        row(ui, "Listen to", |ui| {
            let mut mode = mi.mode;
            egui::ComboBox::from_id_salt("midi_in_mode")
                .width(200.0)
                .selected_text(mode_label(mode))
                .show_ui(ui, |ui| {
                    for m in [
                        MidiInputMode::None,
                        MidiInputMode::All,
                        MidiInputMode::Selected,
                    ] {
                        ui.selectable_value(&mut mode, m, mode_label(m));
                    }
                });
            if mode != mi.mode {
                st.update_show(json!({ "midi_input": { "mode": mode } }));
            }
        });
        if mi.mode == MidiInputMode::Selected {
            row(ui, "Input ports", |ui| {
                ui.vertical(|ui| {
                    let mut values: Vec<String> =
                        inp.midi_ports.iter().map(|p| port_value(show, p)).collect();
                    for p in &mi.ports {
                        if !values.contains(p) {
                            values.push(p.clone());
                        }
                    }
                    if values.is_empty() {
                        hint(ui, "No MIDI input ports found.");
                    }
                    for v in values {
                        let device = show.resolve_midi_port(&v).to_string();
                        let present = inp.midi_ports.contains(&device);
                        let mut on = mi.ports.contains(&v);
                        let mut text = RichText::new(port_label(show, &v));
                        if !present {
                            text =
                                RichText::new(format!("{} (not connected)", port_label(show, &v)))
                                    .color(theme::WARN);
                        }
                        if ui.checkbox(&mut on, text).changed() {
                            let mut ports: Vec<String> =
                                mi.ports.iter().filter(|p| **p != v).cloned().collect();
                            if on {
                                ports.push(v.clone());
                            }
                            st.update_show(json!({ "midi_input": { "ports": ports } }));
                        }
                    }
                });
            });
        }
        row(ui, "MIDI in", |ui| {
            ui.vertical(|ui| {
                match &inp.midi_last {
                    Some(last) if inp.midi_count > 0 => {
                        ui.label(
                            RichText::new(format!("{}  ({})", last.text, last.port))
                                .color(theme::NEXT),
                        );
                        hint(ui, &format!("{} message(s) received", inp.midi_count));
                    }
                    _ => {
                        ui.label(RichText::new("No MIDI received yet").color(theme::MUTED));
                    }
                }
                let open = if inp.midi_open.is_empty() {
                    "no input port open".to_string()
                } else {
                    format!("Open: {}", inp.midi_open.join(", "))
                };
                hint(ui, &open);
            });
        });
        row(ui, "MSC receive", |ui| {
            let mut on = mi.msc.enabled;
            if ui
                .checkbox(&mut on, "Answer MIDI Show Control")
                .on_hover_text("GO (with cue number), STOP, RESUME, ALL_OFF, RESET, LOAD")
                .changed()
            {
                st.update_show(json!({ "midi_input": { "msc": { "enabled": on } } }));
            }
        });
        row(ui, "MSC device ID", |ui| {
            ui.add_enabled_ui(mi.msc.enabled, |ui| {
                if let Some(v) = num_field(
                    ui,
                    &mut st.drafts,
                    "msc_dev",
                    mi.msc.device_id,
                    0,
                    127,
                    50.0,
                ) {
                    st.update_show(json!({ "midi_input": { "msc": { "device_id": v } } }));
                }
            });
            unit(ui, "0-111 (127 = any; all-call is always accepted)");
        });
        row(ui, "MSC format", |ui| {
            let mut f = mi.msc.command_format;
            let label = |f: u8| match f {
                0x10 => "Sound (10)".to_string(),
                0x7F => "All types (7F)".to_string(),
                0x01 => "Lighting (01)".to_string(),
                other => format!("{other:02X}"),
            };
            ui.add_enabled_ui(mi.msc.enabled, |ui| {
                egui::ComboBox::from_id_salt("msc_format")
                    .width(160.0)
                    .selected_text(label(f))
                    .show_ui(ui, |ui| {
                        for x in [0x10u8, 0x7F, 0x01] {
                            ui.selectable_value(&mut f, x, label(x));
                        }
                    });
            });
            if f != mi.msc.command_format {
                st.update_show(json!({ "midi_input": { "msc": { "command_format": f } } }));
            }
        });
    });
    mapping_section(ui, st, s);
}

fn mapping_section(ui: &mut egui::Ui, st: &mut SetupState, s: &UiState) {
    let show = &s.show;
    let mi = &show.midi_input;
    let maps = &mi.mappings;
    heading(ui, "Remote-control mapping");
    hint(
        ui,
        "Incoming MIDI message -> command. Only messages from the ports listened to count.",
    );
    if mi.mode == MidiInputMode::None && !maps.is_empty() {
        hint(ui, "MIDI input is off: choose ports to listen to above.");
    }

    // A message arrived while learning: fill the trigger.
    let learn = learning(ui);
    if let (Some((i, seq)), Some(last)) = (learn, &s.inputs.midi_last) {
        if last.seq > seq {
            if let Some(m) = maps.get(i).and_then(|m| learned(show, m, last)) {
                let mut v = maps.clone();
                v[i] = m;
                st.update_show(json!({ "midi_input": { "mappings": v } }));
                set_learning(ui, None);
            }
        }
    }
    if learn.is_some_and(|(i, _)| i >= maps.len()) {
        set_learning(ui, None);
    }

    let mut new: Option<Vec<MidiMapping>> = None;
    let mut port_values: Vec<String> = show.midi_ports.iter().map(|a| a.name.clone()).collect();
    for p in &s.inputs.midi_ports {
        let v = port_value(show, p);
        if !port_values.contains(&v) {
            port_values.push(v);
        }
    }
    for (i, m) in maps.iter().enumerate() {
        let n = i + 1;
        let edit = |f: &dyn Fn(&mut MidiMapping)| {
            let mut v = maps.clone();
            f(&mut v[i]);
            v
        };
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new(format!("{n}.")).strong());
                ui.label(RichText::new("Port").color(theme::MUTED));
                let mut port = m.port.clone();
                ui.name_inputs(format!("Mapping {n} port"), |ui| {
                    egui::ComboBox::from_id_salt(("map_port", i))
                        .width(150.0)
                        .selected_text(if port.is_empty() {
                            "Any".to_string()
                        } else {
                            port_label(show, &port)
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut port, String::new(), "Any");
                            for v in &port_values {
                                ui.selectable_value(&mut port, v.clone(), port_label(show, v));
                            }
                        });
                });
                if port != m.port {
                    new = Some(edit(&|x| x.port = port.clone()));
                }
                let mut kind = m.kind;
                ui.name_inputs(format!("Mapping {n} type"), |ui| {
                    egui::ComboBox::from_id_salt(("map_kind", i))
                        .width(130.0)
                        .selected_text(kind_label(kind))
                        .show_ui(ui, |ui| {
                            for k in [
                                MidiTriggerKind::NoteOn,
                                MidiTriggerKind::ControlChange,
                                MidiTriggerKind::ProgramChange,
                            ] {
                                ui.selectable_value(&mut kind, k, kind_label(k));
                            }
                        });
                });
                if kind != m.kind {
                    new = Some(edit(&|x| x.kind = kind));
                }
                ui.label(RichText::new("Ch").color(theme::MUTED));
                ui.name_inputs(format!("Mapping {n} channel"), |ui| {
                    if let Some(v) = num_field(
                        ui,
                        &mut st.drafts,
                        &format!("map_ch_{i}"),
                        m.channel,
                        0,
                        16,
                        32.0,
                    ) {
                        new = Some(edit(&|x| x.channel = v));
                    }
                });
                let num_name = match m.kind {
                    MidiTriggerKind::NoteOn => "Note",
                    MidiTriggerKind::ControlChange => "CC",
                    MidiTriggerKind::ProgramChange => "Program",
                };
                ui.label(RichText::new(num_name).color(theme::MUTED));
                ui.name_inputs(format!("Mapping {n} number"), |ui| {
                    if let Some(v) = num_field(
                        ui,
                        &mut st.drafts,
                        &format!("map_no_{i}"),
                        m.number,
                        0,
                        127,
                        40.0,
                    ) {
                        new = Some(edit(&|x| x.number = v));
                    }
                });
                if m.kind != MidiTriggerKind::ProgramChange {
                    ui.label(RichText::new("Min").color(theme::MUTED))
                        .on_hover_text("Minimum velocity / value (CC fires when rising to it)");
                    ui.name_inputs(format!("Mapping {n} minimum"), |ui| {
                        if let Some(v) = num_field(
                            ui,
                            &mut st.drafts,
                            &format!("map_min_{i}"),
                            m.min_value,
                            1,
                            127,
                            36.0,
                        ) {
                            new = Some(edit(&|x| x.min_value = v));
                        }
                    });
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Action").color(theme::MUTED));
                let mut action = m.action;
                ui.name_inputs(format!("Mapping {n} action"), |ui| {
                    egui::ComboBox::from_id_salt(("map_action", i))
                        .width(190.0)
                        .selected_text(action_label(action))
                        .show_ui(ui, |ui| {
                            for a in MidiMapAction::ALL {
                                ui.selectable_value(&mut action, a, action_label(a));
                            }
                        });
                });
                if action != m.action {
                    new = Some(edit(&|x| x.action = action));
                }
                if m.action.needs_cue() {
                    ui.label(RichText::new("Cue no.").color(theme::MUTED));
                    ui.name_inputs(format!("Mapping {n} cue number"), |ui| {
                        if let Some(v) = text_field(
                            ui,
                            &mut st.drafts,
                            &format!("map_cue_{i}"),
                            &m.cue_number,
                            60.0,
                        ) {
                            let v = v.trim().to_string();
                            new = Some(edit(&|x| x.cue_number = v.clone()));
                        }
                    });
                    if !m.cue_number.is_empty() && show.find_by_number(&m.cue_number).is_none() {
                        ui.label(RichText::new("no such cue").color(theme::WARN));
                    }
                }
                let is_learning = learn.is_some_and(|(l, _)| l == i);
                let learn_text = if is_learning {
                    "Waiting for MIDI... (cancel)"
                } else {
                    "Learn"
                };
                if ui
                    .button(learn_text)
                    .on_hover_text("Fill the trigger from the next incoming message")
                    .clicked()
                {
                    set_learning(ui, (!is_learning).then_some((i, s.inputs.midi_count)));
                }
                if small_button(ui, "Remove")
                    .on_hover_text(format!("Remove mapping {n}"))
                    .clicked()
                {
                    let mut v = maps.clone();
                    v.remove(i);
                    new = Some(v);
                    set_learning(ui, None);
                }
            });
        });
    }
    if maps.is_empty() {
        hint(ui, "No mappings: incoming MIDI does nothing.");
    }
    if ui.button("Add mapping").clicked() {
        let mut v = maps.clone();
        v.push(MidiMapping::default());
        new = Some(v);
    }
    if let Some(v) = new {
        st.update_show(json!({ "midi_input": { "mappings": v } }));
    }
}

fn status_text(status: &str) -> (&'static str, egui::Color32) {
    match status {
        "locked" => ("Locked - following", theme::NEXT),
        "locking" => ("Locking...", theme::WARN),
        "freewheel" => ("Input interrupted - freewheeling", theme::PAUSED),
        "lost" => ("Lost - timecode stopped", theme::ERROR),
        "no_signal" => ("No signal", theme::ERROR),
        "stopped" => ("Source stopped", theme::MUTED),
        "held" => ("Held (PANIC / stop) - not following", theme::WARN),
        _ => ("Internal generator", theme::MUTED),
    }
}

pub(super) fn timecode_source_section(ui: &mut egui::Ui, st: &mut SetupState, s: &UiState) {
    let show = &s.show;
    let tc = &show.timecode;
    let input = &tc.input;
    let inp = &s.inputs;
    heading(ui, "Source");
    form(ui, "tc_source", |ui| {
        row(ui, "Timecode source", |ui| {
            let mut src = tc.source;
            let label = |x: TcSource| match x {
                TcSource::Generator => "Internal generator",
                TcSource::External => "External (chase MTC / LTC)",
            };
            egui::ComboBox::from_id_salt("tc_source")
                .width(240.0)
                .selected_text(label(src))
                .show_ui(ui, |ui| {
                    for x in [TcSource::Generator, TcSource::External] {
                        ui.selectable_value(&mut src, x, label(x));
                    }
                });
            if src != tc.source {
                st.update_show(json!({ "timecode": { "source": src } }));
            }
        });
        if tc.source != TcSource::External {
            return;
        }
        row(ui, "Status", |ui| {
            let (text, color) = status_text(&inp.tc_status);
            ui.label(RichText::new(text).color(color).strong());
        });
        row(ui, "Input", |ui| {
            let mut kind = input.kind;
            let label = |k: TcInputKind| match k {
                TcInputKind::Mtc => "MTC (MIDI input)",
                TcInputKind::Ltc => "LTC (audio input)",
            };
            egui::ComboBox::from_id_salt("tc_in_kind")
                .width(200.0)
                .selected_text(label(kind))
                .show_ui(ui, |ui| {
                    for k in [TcInputKind::Mtc, TcInputKind::Ltc] {
                        ui.selectable_value(&mut kind, k, label(k));
                    }
                });
            if kind != input.kind {
                st.update_show(json!({ "timecode": { "input": { "kind": kind } } }));
            }
        });
        match input.kind {
            TcInputKind::Mtc => {
                row(ui, "MTC input port", |ui| {
                    let mut port = input.mtc_port.clone();
                    let mut values: Vec<String> =
                        show.midi_ports.iter().map(|a| a.name.clone()).collect();
                    for p in &inp.midi_ports {
                        let v = port_value(show, p);
                        if !values.contains(&v) {
                            values.push(v);
                        }
                    }
                    let missing = !port.is_empty()
                        && !inp
                            .midi_ports
                            .iter()
                            .any(|p| p == show.resolve_midi_port(&port));
                    let shown = if port.is_empty() {
                        RichText::new("(any listened port)")
                    } else if missing {
                        RichText::new(format!("{} (not connected)", port_label(show, &port)))
                            .color(theme::WARN)
                    } else {
                        RichText::new(port_label(show, &port))
                    };
                    egui::ComboBox::from_id_salt("tc_in_mtc_port")
                        .width(260.0)
                        .selected_text(shown)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut port, String::new(), "(any listened port)");
                            for v in &values {
                                ui.selectable_value(&mut port, v.clone(), port_label(show, v));
                            }
                        });
                    if port != input.mtc_port {
                        st.update_show(json!({ "timecode": { "input": { "mtc_port": port } } }));
                    }
                    if let Some(r) = &inp.mtc_rate {
                        unit(ui, &format!("receiving {r}"));
                    }
                });
            }
            TcInputKind::Ltc => {
                row(ui, "Audio input", |ui| {
                    let mut dev = input.ltc_device.clone();
                    let devices = inp.audio_inputs.clone().unwrap_or_default();
                    let name = |id: &Option<String>| match id {
                        None => "(system default input)".to_string(),
                        Some(id) => devices
                            .iter()
                            .find(|d| &d.id == id)
                            .map(|d| format!("{} ({} ch)", d.name, d.channels))
                            .unwrap_or_else(|| format!("{id} (not connected)")),
                    };
                    egui::ComboBox::from_id_salt("tc_in_ltc_dev")
                        .width(260.0)
                        .selected_text(name(&dev))
                        .show_ui(ui, |ui| {
                            ui.selectable_value(&mut dev, None, name(&None));
                            for d in &devices {
                                ui.selectable_value(
                                    &mut dev,
                                    Some(d.id.clone()),
                                    name(&Some(d.id.clone())),
                                );
                            }
                        });
                    if dev != input.ltc_device {
                        st.update_show(json!({ "timecode": { "input": { "ltc_device": dev } } }));
                    }
                    if inp.audio_inputs.is_none() {
                        unit(ui, "scanning...");
                    }
                });
                row(ui, "Input channel", |ui| {
                    if let Some(v) = num_field(
                        ui,
                        &mut st.drafts,
                        "ltc_in_ch",
                        input.ltc_channel + 1,
                        1,
                        64,
                        50.0,
                    ) {
                        st.update_show(
                            json!({ "timecode": { "input": { "ltc_channel": v - 1 } } }),
                        );
                    }
                    unit(ui, "1 = first");
                });
                row(ui, "Input level", |ui| {
                    let text = match (inp.ltc_open, inp.ltc_level_db) {
                        (false, _) => RichText::new("input not open").color(theme::ERROR),
                        (true, Some(db)) if db > -60.0 => {
                            RichText::new(format!("{db:.1} dBFS peak")).color(theme::NEXT)
                        }
                        (true, _) => RichText::new("silence").color(theme::WARN),
                    };
                    ui.label(text);
                });
            }
        }
        row(ui, "Lock after", |ui| {
            if let Some(v) = num_field(
                ui,
                &mut st.drafts,
                "tc_lock",
                input.lock_frames,
                1,
                50,
                50.0,
            ) {
                st.update_show(json!({ "timecode": { "input": { "lock_frames": v } } }));
            }
            unit(ui, "valid frames in a row");
        });
        row(ui, "Freewheel", |ui| {
            if let Some(v) = num_field(
                ui,
                &mut st.drafts,
                "tc_freewheel",
                input.freewheel_ms,
                0,
                30_000,
                70.0,
            ) {
                st.update_show(json!({ "timecode": { "input": { "freewheel_ms": v } } }));
            }
            unit(ui, "ms without input, then lost / stopped");
        });
    });
    if tc.source == TcSource::External {
        hint(
            ui,
            "The frame rate below must match the source. Timecode cues and generator \
             commands do not move the clock while chasing; LTC / MTC outputs send the \
             chased timecode.",
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn learn_fills_the_trigger() {
        let mut show = Show::default();
        show.midi_ports.push(sm_core::model::MidiPortAlias {
            name: "Pad".into(),
            device: "USB Pad".into(),
        });
        let m = MidiMapping {
            min_value: 64,
            action: MidiMapAction::Next,
            ..MidiMapping::default()
        };
        let last = MidiLast {
            seq: 3,
            port: "USB Pad".into(),
            text: String::new(),
            kind: "control_change".into(),
            channel: 2,
            number: 21,
            value: 127,
        };
        let got = learned(&show, &m, &last).unwrap();
        assert_eq!(got.port, "Pad");
        assert_eq!(got.kind, MidiTriggerKind::ControlChange);
        assert_eq!((got.channel, got.number, got.min_value), (2, 21, 64));
        assert_eq!(got.action, MidiMapAction::Next);
        let off = MidiLast {
            kind: "note_off".into(),
            ..last
        };
        assert!(learned(&show, &m, &off).is_none());
    }
}
