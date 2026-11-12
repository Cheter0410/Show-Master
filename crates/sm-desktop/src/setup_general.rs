//! Setup tabs "General" (show name, safety, display, timer alarms) and
//! "About / Updates".

use super::{form, heading, hint, num_field, row, small_button, text_field, unit, SetupState};
use crate::model::UiState;
use crate::theme;
use crate::util::parse_time;
use egui::RichText;
use serde_json::json;
use sm_core::model::TimerAlarm;

/// `h:mm:ss` for an alarm time.
pub(super) fn fmt_alarm(secs: f64) -> String {
    let t = secs.max(0.0) as u64;
    format!("{}:{:02}:{:02}", t / 3600, (t / 60) % 60, t % 60)
}

pub(super) fn general_tab(ui: &mut egui::Ui, st: &mut SetupState, s: &UiState) {
    let set = &s.show.settings;
    heading(ui, "Show");
    form(ui, "general_show", |ui| {
        row(ui, "Show name", |ui| {
            if let Some(v) = text_field(ui, &mut st.drafts, "show_name", &s.show.name, 260.0) {
                let v = v.trim().to_string();
                if !v.is_empty() {
                    st.update_show(json!({ "name": v }));
                }
            }
        });
    });

    heading(ui, "Safety");
    form(ui, "general_safety", |ui| {
        row(ui, "GO debounce", |ui| {
            if let Some(v) = num_field(
                ui,
                &mut st.drafts,
                "go_debounce",
                set.go_debounce_ms,
                0,
                5000,
                70.0,
            ) {
                st.update_settings(json!({ "go_debounce_ms": v }));
            }
            unit(ui, "ms - a second GO within this time is ignored");
        });
        row(ui, "Stop-all fade", |ui| {
            if let Some(v) = num_field(
                ui,
                &mut st.drafts,
                "stop_fade",
                set.stop_fade_ms,
                0,
                60_000,
                70.0,
            ) {
                st.update_settings(json!({ "stop_fade_ms": v }));
            }
            unit(ui, "ms");
        });
        row(ui, "Panic fade", |ui| {
            if let Some(v) = num_field(
                ui,
                &mut st.drafts,
                "panic_fade",
                set.panic_fade_ms,
                0,
                60_000,
                70.0,
            ) {
                st.update_settings(json!({ "panic_fade_ms": v }));
            }
            unit(ui, "ms - 0 = cut immediately");
        });
        row(ui, "Panic", |ui| {
            let mut on = set.panic_double_tap;
            if ui.checkbox(&mut on, "Require a double press").changed() {
                st.update_settings(json!({ "panic_double_tap": on }));
            }
        });
        row(ui, "Double-press window", |ui| {
            ui.add_enabled_ui(set.panic_double_tap, |ui| {
                if let Some(v) = num_field(
                    ui,
                    &mut st.drafts,
                    "double_tap",
                    set.double_tap_ms,
                    150,
                    3000,
                    70.0,
                ) {
                    st.update_settings(json!({ "double_tap_ms": v }));
                }
            });
            unit(ui, "ms");
        });
        row(ui, "Quit protection", |ui| {
            let mut on = set.quit_lock;
            if ui
                .checkbox(&mut on, "Ask before quitting in Live mode")
                .changed()
            {
                st.update_settings(json!({ "quit_lock": on }));
            }
        });
        row(ui, "Display", |ui| {
            let mut on = set.high_contrast;
            if ui.checkbox(&mut on, "High contrast").changed() {
                st.update_settings(json!({ "high_contrast": on }));
            }
        });
    });

    heading(ui, "Show timer alarms");
    hint(
        ui,
        "Shown on all screens when the show clock reaches the time (h:mm:ss).",
    );
    let alarms = &set.timer_alarms;
    let mut new_alarms: Option<Vec<TimerAlarm>> = None;
    egui::Grid::new("alarms")
        .num_columns(3)
        .spacing([12.0, 6.0])
        .show(ui, |ui| {
            for (i, a) in alarms.iter().enumerate() {
                ui.name_inputs(format!("Alarm {} time", i + 1), |ui| {
                    if let Some(v) = text_field(
                        ui,
                        &mut st.drafts,
                        &format!("alarm_at_{i}"),
                        &fmt_alarm(a.at),
                        80.0,
                    ) {
                        match parse_time(&v) {
                            Some(t) => {
                                let mut list = alarms.clone();
                                list[i].at = t.round();
                                new_alarms = Some(list);
                            }
                            None => st.error("Alarm time: expected h:mm:ss"),
                        }
                    }
                });
                ui.name_inputs(format!("Alarm {} label", i + 1), |ui| {
                    if let Some(v) = text_field(
                        ui,
                        &mut st.drafts,
                        &format!("alarm_label_{i}"),
                        &a.label,
                        220.0,
                    ) {
                        let mut list = alarms.clone();
                        list[i].label = v;
                        new_alarms = Some(list);
                    }
                });
                if small_button(ui, "Remove")
                    .on_hover_text(format!("Remove alarm {}", a.label))
                    .clicked()
                {
                    let mut list = alarms.clone();
                    list.remove(i);
                    new_alarms = Some(list);
                }
                ui.end_row();
            }
        });
    if alarms.is_empty() {
        hint(ui, "No alarms.");
    }
    if ui.button("Add alarm").clicked() {
        let mut list = alarms.clone();
        let at = list.iter().map(|a| a.at).fold(0.0, f64::max) + 3600.0;
        list.push(TimerAlarm {
            at,
            label: "Alarm".into(),
        });
        new_alarms = Some(list);
    }
    if let Some(mut list) = new_alarms {
        list.sort_by(|a, b| a.at.total_cmp(&b.at));
        st.update_settings(json!({ "timer_alarms": list }));
    }
}

/// Where the log file normally is (unless `--log-dir` was given).
fn log_hint() -> String {
    if cfg!(target_os = "windows") {
        r"%LOCALAPPDATA%\ShowMaster\logs\showmaster.log".into()
    } else if cfg!(target_os = "macos") {
        "~/Library/Logs/ShowMaster/showmaster.log".into()
    } else {
        "$XDG_STATE_HOME/showmaster/showmaster.log (default ~/.local/state/showmaster/)".into()
    }
}

pub(super) fn about_tab(ui: &mut egui::Ui, s: &UiState, out: &mut SetupState) {
    let u = &s.update;
    heading(ui, "Show Master");
    form(ui, "about", |ui| {
        row(ui, "Version", |ui| {
            let v = if s.version.is_empty() {
                &u.current
            } else {
                &s.version
            };
            ui.label(RichText::new(v).strong());
        });
        row(ui, "Updates", |ui| {
            let (text, color) = match u.state.as_str() {
                "disabled" => ("Update check is disabled".to_string(), theme::MUTED),
                "checking" => ("Checking…".to_string(), theme::LOADED),
                "current" => ("Show Master is up to date".to_string(), theme::NEXT),
                "offline" => ("Update server not reachable".to_string(), theme::WARN),
                "available" => (
                    format!(
                        "Version {} is available",
                        u.version.clone().unwrap_or_default()
                    ),
                    theme::NEXT,
                ),
                "downloading" => (
                    format!(
                        "Downloading… {}%",
                        (100 * u.done).checked_div(u.total).unwrap_or(0)
                    ),
                    theme::LOADED,
                ),
                "installing" => ("Installing…".to_string(), theme::LOADED),
                "restarting" => ("Restarting…".to_string(), theme::LOADED),
                "failed" => (
                    format!("Update failed: {}", u.error.clone().unwrap_or_default()),
                    theme::ERROR,
                ),
                "" => ("Unknown".to_string(), theme::MUTED),
                other => (other.to_string(), theme::MUTED),
            };
            ui.label(RichText::new(text).color(color));
        });
        row(ui, "", |ui| {
            let busy = matches!(
                u.state.as_str(),
                "checking" | "downloading" | "installing" | "restarting"
            );
            if ui
                .add_enabled(!busy, egui::Button::new("Check for updates now"))
                .clicked()
            {
                out.send(json!({ "cmd": "check_update" }));
            }
            if !u.download_page.is_empty() {
                ui.hyperlink_to("Download page", &u.download_page);
            }
        });
        if let Some(err) = u.error.as_ref().filter(|_| u.state != "failed") {
            row(ui, "Last error", |ui| {
                ui.label(RichText::new(err).color(theme::ERROR));
            });
        }
    });
    if let Some(notes) = u
        .notes
        .as_ref()
        .filter(|n| !n.trim().is_empty() && u.state == "available")
    {
        heading(ui, "What's new");
        ui.label(notes);
    }
    heading(ui, "Logs");
    hint(ui, "The log file helps with support requests:");
    ui.monospace(s.log_file.clone().unwrap_or_else(log_hint));
    ui.horizontal(|ui| {
        if s.log_file.is_some() && ui.button("Show log folder").clicked() {
            out.outbox_push(super::Out::OpenLogFolder);
        }
        if ui
            .button("Export diagnostics…")
            .on_hover_text("Log, system and show state in one text file")
            .clicked()
        {
            out.outbox_push(super::Out::ExportDiagnostics);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alarm_times() {
        assert_eq!(fmt_alarm(3600.0), "1:00:00");
        assert_eq!(fmt_alarm(83.0), "0:01:23");
        assert_eq!(parse_time(&fmt_alarm(5025.0)), Some(5025.0));
    }
}
