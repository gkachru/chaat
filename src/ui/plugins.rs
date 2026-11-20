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
    drop_stale_secret_buffers(app, ui);
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
        if let Some(url) = state.and_then(|s| s.info.docs_url.as_ref()) {
            // The host shows next to the link, so the person sees where it
            // leads before clicking.
            ui.horizontal(|ui| {
                if ui.link("About this plugin").clicked() {
                    actions.push(Action::OpenUrl(url.clone()));
                }
                if let Some(host) = link_host(url) {
                    ui.label(
                        egui::RichText::new(host)
                            .font(theme::regular(12.0))
                            .color(palette.dim),
                    );
                }
            });
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
        if let Some(hash) = state.and_then(|s| s.file_changed.as_deref()) {
            file_changed_notice(app, ui, entry, hash, actions);
        }
        if let Some(state) = state
            && !state.pending_needs.is_empty()
        {
            pending_notice(app, ui, entry, &state.pending_needs, actions);
        }
        receives_line(app, ui, entry);
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
            if state.running && app.plugin_receives(&entry.id).contains(&"accounts") {
                account_rows(app, ui, entry, state, actions);
            }
        }
    }
    ui.add_space(10.0);
    if widgets::secondary_button(ui, &palette, "Add plugin\u{2026}").clicked() {
        actions.push(Action::AddPlugin);
    }
}

/// Chaat's words for what one kind of data is.
fn need_words(need: &str) -> &'static str {
    match need {
        "accounts" => "your accounts' IDs and sign-in names",
        "presence" => "your presence (availability and activity)",
        "options" => "its own settings below",
        _ => "something Chaat does not know",
    }
}

/// "a", "a and b", "a, b and c".
fn join_plainly(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// What the plugin gets from Chaat now, in Chaat's words.
fn receives_line(app: &App, ui: &mut egui::Ui, entry: &PluginEntry) {
    let palette = app.palette;
    let words: Vec<&str> = app
        .plugin_receives(&entry.id)
        .into_iter()
        .map(need_words)
        .collect();
    let text = if words.is_empty() {
        "nothing".to_owned()
    } else {
        join_plainly(&words)
    };
    ui.add(
        egui::Label::new(
            egui::RichText::new(format!("Receives: {text}"))
                .font(theme::regular(13.0))
                .color(palette.secondary),
        )
        .wrap(),
    );
}

/// A boxed notice that stands apart from the plugin's own text.
fn notice(ui: &mut egui::Ui, palette: &theme::Palette, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.warning))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(10))
        .show(ui, add_contents);
    ui.add_space(4.0);
}

/// The needs the plugin declared that the person has not answered yet.
fn pending_notice(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    pending: &[&'static str],
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    notice(ui, &palette, |ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "Plugin \u{201C}{}\u{201D} asks for:",
                    app.plugin_name(&entry.id)
                ))
                .font(theme::medium(13.0))
                .color(palette.text),
            )
            .wrap(),
        );
        for need in pending {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(need_words(need))
                        .font(theme::regular(13.0))
                        .color(palette.secondary),
                )
                .wrap(),
            );
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if widgets::primary_button(ui, &palette, "Allow").clicked() {
                actions.push(Action::ApprovePluginNeeds(entry.id.clone()));
            }
            if widgets::secondary_button(ui, &palette, "Don't allow").clicked() {
                actions.push(Action::DenyPluginNeeds(entry.id.clone()));
            }
        });
    });
}

/// The plugin's file no longer matches the one the person added.
fn file_changed_notice(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    hash: &str,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    notice(ui, &palette, |ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "The plugin file changed since you added it. Run the new file only if you trust it.",
                )
                .font(theme::regular(13.0))
                .color(palette.text),
            )
            .wrap(),
        );
        let short: String = hash.chars().take(12).collect();
        ui.label(
            egui::RichText::new(format!("New file fingerprint: {short}"))
                .font(theme::regular(12.0))
                .color(palette.dim),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if widgets::secondary_button(ui, &palette, "Run the new file").clicked() {
                actions.push(Action::TrustPluginFile(entry.id.clone()));
            }
            if widgets::secondary_button(ui, &palette, "Keep it off").clicked() {
                actions.push(Action::KeepPluginOff(entry.id.clone()));
            }
        });
    });
}

/// The host of an `https://` link: no scheme, user info, port or path.
fn link_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = if authority.starts_with('[') {
        authority.split_inclusive(']').next()?
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_owned())
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

/// Typed but unsaved secrets, by plugin and option, in one egui memory slot so
/// they can all be dropped at once.
type SecretBuffers = std::collections::HashMap<(String, String), String>;

fn secret_buffers_id() -> egui::Id {
    egui::Id::new("plugin-secret-buffers")
}

/// Drops every unsaved secret the person typed. The page calls it when it was
/// not drawn in the frame before; Settings may call it when it closes.
pub fn forget_secret_buffers(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        d.remove_temp::<SecretBuffers>(secret_buffers_id());
        // The text edit's own state (cursor, undo history) can hold the typed
        // text too, so it goes with the buffers.
        for id in d
            .remove_temp::<SecretFields>(secret_fields_id())
            .unwrap_or_default()
        {
            d.remove::<egui::text_edit::TextEditState>(id);
        }
    });
}

/// The ids of the secret fields drawn so far, so their edit state can be
/// dropped even when the buffer is empty.
type SecretFields = Vec<egui::Id>;

fn secret_fields_id() -> egui::Id {
    egui::Id::new("plugin-secret-fields")
}

fn forget_secret_edit_state(ctx: &egui::Context, field: egui::Id) {
    ctx.data_mut(|d| d.remove::<egui::text_edit::TextEditState>(field));
}

/// An unsaved secret does not survive leaving the page: if the page was not
/// drawn in the previous frame, the buffers go. Buffers of plugins or options
/// that no longer exist go too.
fn drop_stale_secret_buffers(app: &App, ui: &egui::Ui) {
    let frame = ui.ctx().cumulative_frame_nr();
    let seen = egui::Id::new("plugin-page-last-frame");
    let last = ui.data(|d| d.get_temp::<u64>(seen));
    if last.is_none_or(|last| frame > last.saturating_add(1)) {
        forget_secret_buffers(ui.ctx());
    }
    let live = |plugin: &str, option: &str| {
        app.plugins.get(plugin).is_some_and(|state| {
            state
                .options
                .iter()
                .any(|o| o.id == option && o.kind == OptionKind::Secret)
        })
    };
    ui.data_mut(|d| {
        d.insert_temp(seen, frame);
        if let Some(buffers) = d.get_temp::<SecretBuffers>(secret_buffers_id()) {
            let mut buffers = buffers;
            buffers.retain(|(plugin, option), _| live(plugin, option));
            d.insert_temp(secret_buffers_id(), buffers);
        }
    });
}

fn secret_buffer(ui: &egui::Ui, plugin: &str, option: &str) -> String {
    ui.data(|d| d.get_temp::<SecretBuffers>(secret_buffers_id()))
        .and_then(|buffers| {
            buffers
                .get(&(plugin.to_owned(), option.to_owned()))
                .cloned()
        })
        .unwrap_or_default()
}

fn set_secret_buffer(ui: &egui::Ui, plugin: &str, option: &str, value: String) {
    ui.data_mut(|d| {
        let mut buffers = d
            .get_temp::<SecretBuffers>(secret_buffers_id())
            .unwrap_or_default();
        let key = (plugin.to_owned(), option.to_owned());
        if value.is_empty() {
            buffers.remove(&key);
        } else {
            buffers.insert(key, value);
        }
        d.insert_temp(secret_buffers_id(), buffers);
    });
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
            let mut buffer = secret_buffer(ui, &entry.id, &option.id);
            let field = buffer_id(&entry.id, &option.id, "secret");
            ui.data_mut(|d| {
                let mut fields = d
                    .get_temp::<SecretFields>(secret_fields_id())
                    .unwrap_or_default();
                if !fields.contains(&field) {
                    fields.push(field);
                    d.insert_temp(secret_fields_id(), fields);
                }
            });
            ui.horizontal(|ui| {
                ui.add(
                    widgets::text_field(&mut buffer, "New value", theme::regular(14.0))
                        .id(field)
                        .password(true)
                        .desired_width((ui.available_width() - 150.0).max(80.0)),
                );
                let typed = !buffer.is_empty();
                if ui.add_enabled(typed, egui::Button::new("Save")).clicked() {
                    forget_secret_edit_state(ui.ctx(), field);
                    actions.push(Action::SetPluginSecret {
                        plugin: entry.id.clone(),
                        option: option.id.clone(),
                        value: Some(std::mem::take(&mut buffer)),
                    });
                }
                if ui.add_enabled(saved, egui::Button::new("Clear")).clicked() {
                    buffer.clear();
                    forget_secret_edit_state(ui.ctx(), field);
                    actions.push(Action::SetPluginSecret {
                        plugin: entry.id.clone(),
                        option: option.id.clone(),
                        value: None,
                    });
                }
            });
            set_secret_buffer(ui, &entry.id, &option.id, buffer);
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
