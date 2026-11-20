//! Settings › Plugins: the plugins Chaat launches. The words about what a
//! plugin is are Chaat's; the plugin's own name and description show apart.

use super::settings::{group, note};
use super::widgets::{self, PointerControls as _};
use crate::app::{App, PluginState};
use crate::model::Action;
use crate::plugin::protocol::{OptionKind, OptionSpec};
use crate::plugin::registry;
use crate::settings::PluginEntry;
use crate::theme;

pub fn page(app: &App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    // Chaat's own words, whatever a plugin says about itself.
    note(
        ui,
        &palette,
        "Plugins are separate programs you install, not part of Chaat. Chaat starts the ones you turn on and shows what they report. It never sends a plugin your messages or sign-in tokens. Check what a plugin does before you use it.",
    );
    group(ui, &palette, "Plugins");
    if app.settings.plugins.is_empty() {
        note(ui, &palette, "No plugins added.");
    }
    let owner = app.call_alerts_owner();
    for entry in &app.settings.plugins {
        let state = app.plugins.get(&entry.id);
        ui.add_space(6.0);
        heading(app, ui, entry);
        if let Some(description) = state.and_then(|s| s.info.description.as_ref()) {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(description)
                        .font(theme::regular(13.0))
                        .color(palette.secondary),
                )
                .wrap(),
            );
        }
        if let Some(url) = state.and_then(|s| s.info.docs_url.as_ref())
            && ui.link("About this plugin").clicked()
        {
            actions.push(Action::OpenUrl(url.clone()));
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(entry.path.display().to_string())
                    .font(theme::regular(12.0))
                    .color(palette.dim),
            )
            .truncate(),
        );
        status(app, ui, entry, state);
        let offers_alerts = state.is_some_and(|s| s.offers.contains(&registry::CALL_ALERTS));
        if offers_alerts && entry.enabled && owner.as_deref().is_some_and(|o| o != entry.id) {
            note(
                ui,
                &palette,
                "Another plugin provides call alerts, so this one's rings are ignored.",
            );
        }
        ui.horizontal(|ui| {
            let mut enabled = entry.enabled;
            if ui.checkbox_hand(&mut enabled, "Turn on").changed() {
                actions.push(Action::SetPluginEnabled(entry.id.clone(), enabled));
            }
            if widgets::secondary_button(ui, &palette, "Remove").clicked() {
                actions.push(Action::RemovePlugin(entry.id.clone()));
            }
        });
        if let Some(state) = state {
            for option in &state.options {
                option_row(app, ui, entry, option, actions);
            }
            if state.running && state.needs.contains(&"accounts") {
                account_rows(app, ui, entry, state, actions);
            }
        }
    }
    ui.add_space(10.0);
    if widgets::secondary_button(ui, &palette, "Add plugin\u{2026}").clicked() {
        actions.push(Action::AddPlugin);
    }
}

/// The plugin's own name, quoted so it reads as the plugin's, not Chaat's.
fn heading(app: &App, ui: &mut egui::Ui, entry: &PluginEntry) {
    let palette = app.palette;
    ui.add(
        egui::Label::new(
            egui::RichText::new(format!(
                "Plugin: \u{201C}{}\u{201D}",
                app.plugin_name(&entry.id)
            ))
            .font(theme::medium(13.0))
            .color(palette.text),
        )
        .truncate(),
    );
}

fn status(app: &App, ui: &mut egui::Ui, entry: &PluginEntry, state: Option<&PluginState>) {
    let palette = app.palette;
    let (status, color) = if !entry.enabled {
        ("Off", palette.dim)
    } else if state.is_some_and(|s| s.running) {
        ("Running", palette.secondary)
    } else if let Some(message) = state.and_then(|s| s.message.as_deref()) {
        (message, palette.warning)
    } else {
        ("Not running", palette.warning)
    };
    ui.add(
        egui::Label::new(
            egui::RichText::new(status)
                .font(theme::regular(13.0))
                .color(color),
        )
        .wrap(),
    );
}

/// A typing buffer lives in egui memory, per plugin and option.
fn buffer_id(plugin: &str, option: &str, what: &str) -> egui::Id {
    egui::Id::new(("plugin-option", what, plugin, option))
}

fn labelled(ui: &mut egui::Ui, palette: &theme::Palette, label: &str) {
    ui.label(
        egui::RichText::new(label)
            .font(theme::regular(13.0))
            .color(palette.secondary),
    );
}

fn option_row(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    option: &OptionSpec,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    let current = entry
        .options
        .get(&option.id)
        .or(option.default.as_ref())
        .cloned();
    let set = |value: serde_json::Value| Action::SetPluginOption {
        plugin: entry.id.clone(),
        option: option.id.clone(),
        value,
    };
    match option.kind {
        OptionKind::Toggle => {
            let mut on = current.as_ref().and_then(|v| v.as_bool()).unwrap_or(false);
            if ui.checkbox_hand(&mut on, &option.label).changed() {
                actions.push(set(on.into()));
            }
        }
        OptionKind::Choice => {
            let selected = current
                .as_ref()
                .and_then(|v| v.as_str())
                .and_then(|value| option.choices.iter().find(|c| c.value == value));
            ui.horizontal(|ui| {
                labelled(ui, &palette, &option.label);
                egui::ComboBox::from_id_salt(buffer_id(&entry.id, &option.id, "choice"))
                    .selected_text(selected.map_or("Not set", |c| c.label.as_str()))
                    .show_ui(ui, |ui| {
                        for choice in &option.choices {
                            let on = selected.is_some_and(|s| s.value == choice.value);
                            if ui.selectable_label(on, &choice.label).clicked() && !on {
                                actions.push(set(choice.value.clone().into()));
                            }
                        }
                    });
            });
        }
        OptionKind::Text => {
            let saved = current
                .as_ref()
                .and_then(|v| v.as_str())
                .unwrap_or_default()
                .to_owned();
            labelled(ui, &palette, &option.label);
            let id = buffer_id(&entry.id, &option.id, "text");
            let editing = ui
                .data(|d| d.get_temp::<bool>(id.with("editing")))
                .unwrap_or(false);
            let mut buffer = if editing {
                ui.data(|d| d.get_temp::<String>(id)).unwrap_or_default()
            } else {
                saved.clone()
            };
            let response = ui.add(
                widgets::text_field(&mut buffer, &option.label, theme::regular(14.0))
                    .id(id.with("field")),
            );
            // Enter and clicking away both end the edit and commit it.
            if response.lost_focus() && buffer != saved {
                actions.push(set(buffer.clone().into()));
            }
            let focused = response.has_focus();
            ui.data_mut(|d| {
                d.insert_temp(id.with("editing"), focused);
                d.insert_temp(id, buffer);
            });
        }
        OptionKind::Secret => {
            let saved = entry.secrets.contains(&option.id);
            labelled(ui, &palette, &option.label);
            ui.label(
                egui::RichText::new(if saved { "Saved" } else { "Not set" })
                    .font(theme::regular(13.0))
                    .color(palette.dim),
            );
            // The stored value is never read back into the interface.
            let id = buffer_id(&entry.id, &option.id, "secret");
            let mut buffer = ui.data(|d| d.get_temp::<String>(id)).unwrap_or_default();
            ui.horizontal(|ui| {
                ui.add(
                    widgets::text_field(&mut buffer, "New value", theme::regular(14.0))
                        .password(true)
                        .desired_width((ui.available_width() - 150.0).max(80.0)),
                );
                let typed = !buffer.is_empty();
                if ui.add_enabled(typed, egui::Button::new("Save")).clicked() {
                    actions.push(Action::SetPluginSecret {
                        plugin: entry.id.clone(),
                        option: option.id.clone(),
                        value: Some(std::mem::take(&mut buffer)),
                    });
                }
                if ui.add_enabled(saved, egui::Button::new("Clear")).clicked() {
                    actions.push(Action::SetPluginSecret {
                        plugin: entry.id.clone(),
                        option: option.id.clone(),
                        value: None,
                    });
                }
            });
            ui.data_mut(|d| d.insert_temp(id, buffer));
        }
    }
}

/// One row per Chaat account: whether the plugin listens for it or waits for
/// a sign-in, with a button to ask for a fresh code.
fn account_rows(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    state: &PluginState,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    for account in app.plugin_accounts() {
        let waiting = state
            .sign_in
            .iter()
            .any(|s| s.account.as_deref() == Some(account.id.as_str()));
        let listening = state.listening.contains(&account.id);
        let name = if account.username.is_empty() {
            account.id.clone()
        } else {
            account.username.clone()
        };
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(name)
                        .font(theme::regular(13.0))
                        .color(palette.text),
                )
                .truncate(),
            );
            let (text, color) = if listening {
                ("Listening for calls", palette.success)
            } else if waiting {
                ("Waiting for sign-in", palette.warning)
            } else {
                ("Connecting\u{2026}", palette.dim)
            };
            ui.label(
                egui::RichText::new(text)
                    .font(theme::regular(12.0))
                    .color(color),
            );
            if waiting && widgets::secondary_button(ui, &palette, "New code").clicked() {
                actions.push(Action::PluginSignIn {
                    plugin: entry.id.clone(),
                    account: account.id.clone(),
                });
            }
        });
    }
}
