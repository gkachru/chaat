//! One plugin's page. Section order is fixed: title, what needs the person,
//! status, what the plugin gets from Chaat, its settings (the plugin's own
//! words, in a card under a caption), accounts, and the folded file section.
//! Chaat's words and the plugin's stay apart: the plugin's name and
//! description sit at the top, its option texts under "from the plugin".

use super::super::widgets::{self, PointerControls as _};
use super::super::widgets::{group, note};
use super::options::{OptionsCx, options_block};
use super::{file_changed_items, need_words, notice, pending_items};
use crate::app::{App, PluginState};
use crate::model::{Action, PluginAction, SettingsPage};
use crate::plugin::protocol::OptionSpec;
use crate::plugin::registry::Capability;
use crate::settings::PluginEntry;
use crate::theme;

/// Scratch state: the person pressed "Remove plugin…" and has not answered.
fn confirm_id(id: &str) -> egui::Id {
    egui::Id::new(("plugin-remove-confirm", id))
}

fn set_confirming(ui: &egui::Ui, id: &str, on: bool) {
    ui.data_mut(|d| d.insert_temp(confirm_id(id), on));
}

/// Drops the Remove confirmation for `id`.
pub(super) fn forget_confirm(ui: &egui::Ui, id: &str) {
    set_confirming(ui, id, false);
}

/// Drops the Remove confirmation and the turn-off question for `id`.
pub(super) fn forget_questions(ui: &egui::Ui, id: &str) {
    ui.data_mut(|d| {
        d.remove::<bool>(confirm_id(id));
        d.remove::<bool>(ask_off_id(id));
    });
}

fn ask_off_id(id: &str) -> egui::Id {
    egui::Id::new(("plugin-turn-off-ask", id))
}

fn set_asking_off(ui: &egui::Ui, id: &str, on: bool) {
    ui.data_mut(|d| d.insert_temp(ask_off_id(id), on));
}

/// A switch change: turning off a plugin that provides something asks first
/// (the question shows under the row or title); anything else applies.
pub(super) fn request_enabled(
    app: &App,
    ui: &egui::Ui,
    id: &str,
    on: bool,
    actions: &mut Vec<Action>,
) {
    if !on && app.turn_off_question(id).is_some() {
        set_asking_off(ui, id, true);
    } else {
        actions.push(Action::Plugin(PluginAction::SetEnabled(id.to_owned(), on)));
    }
}

/// The turn-off question in place, with Turn off (danger) and Cancel.
pub(super) fn turn_off_ask(app: &App, ui: &mut egui::Ui, id: &str, actions: &mut Vec<Action>) {
    let asking = ui
        .data(|d| d.get_temp::<bool>(ask_off_id(id)))
        .unwrap_or(false);
    if !asking {
        return;
    }
    let enabled = app.settings.plugins.iter().any(|e| e.id == id && e.enabled);
    let Some(question) = app.turn_off_question(id).filter(|_| enabled) else {
        set_asking_off(ui, id, false);
        return;
    };
    let palette = app.palette;
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.add(
                egui::Label::new(
                    egui::RichText::new(question)
                        .font(theme::regular(13.0))
                        .color(palette.text),
                )
                .wrap(),
            );
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if danger_button(ui, &palette, "Turn off").clicked() {
                    set_asking_off(ui, id, false);
                    actions.push(Action::Plugin(PluginAction::SetEnabled(
                        id.to_owned(),
                        false,
                    )));
                }
                if widgets::secondary_button(ui, &palette, "Cancel").clicked() {
                    set_asking_off(ui, id, false);
                }
            });
        });
}

fn danger_button(ui: &mut egui::Ui, palette: &theme::Palette, text: &str) -> egui::Response {
    let button = egui::Button::new(
        egui::RichText::new(text)
            .font(theme::medium(13.0))
            .color(palette.danger),
    )
    .fill(palette.surface)
    .stroke(egui::Stroke::new(1.0, palette.danger))
    .corner_radius(8)
    .min_size(egui::vec2(0.0, 32.0));
    ui.add(button)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

pub(super) fn plugin_page(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    let state = app.plugins.get(&entry.id);
    // Esc does the same; ui/keys.rs sends it.
    if ui.link("\u{2039} Plugins").clicked() {
        forget_confirm(ui, &entry.id);
        set_asking_off(ui, &entry.id, false);
        actions.push(Action::Plugin(PluginAction::BackToPlugins));
    }
    ui.add_space(6.0);
    title(app, ui, entry, actions);
    turn_off_ask(app, ui, &entry.id, actions);
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
        // The host shows next to the link, so the person sees where it leads
        // before clicking.
        ui.horizontal_wrapped(|ui| {
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
    needs_you(app, ui, entry, state, actions);
    status_section(app, ui, entry, state, actions);
    gets_from_chaat(app, ui, entry);
    if let Some(state) = state {
        if !state.options.is_empty() {
            settings_card(app, ui, entry, state, actions);
        }
        if state.running && app.plugin_receives(&entry.id).contains(&"accounts") {
            group(ui, &palette, "Accounts");
            account_rows(app, ui, entry, state, actions);
        }
    }
    if state.is_some_and(|s| s.running) {
        super::super::agents::access_section(ui, app, &entry.id, actions);
    }
    file_section(app, ui, entry, actions);
    ui.add_space(14.0);
}

/// The plugin's name (wraps) and its switch on one line.
fn title(app: &App, ui: &mut egui::Ui, entry: &PluginEntry, actions: &mut Vec<Action>) {
    let palette = app.palette;
    egui::Sides::new().shrink_left().show(
        ui,
        |ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(app.plugin_name(&entry.id))
                        .font(theme::bold(20.0))
                        .color(palette.text),
                )
                .wrap(),
            );
        },
        |ui| {
            let mut enabled = entry.enabled;
            if ui.checkbox_hand(&mut enabled, "Turn on").changed() {
                request_enabled(app, ui, &entry.id, enabled, actions);
            }
        },
    );
}

/// One callout for everything that needs the person, items stacked: file
/// changed, then pending needs, then required settings still unset.
fn needs_you(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    state: Option<&PluginState>,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    let file = state.and_then(|s| s.file_changed.as_deref());
    let pending: &[&'static str] = state.map_or(&[], |s| s.pending_needs.as_slice());
    let missing = !app.plugin_missing_required(&entry.id).is_empty();
    if file.is_none() && pending.is_empty() && !missing {
        return;
    }
    notice(ui, &palette, |ui| {
        ui.label(
            egui::RichText::new("NEEDS YOU")
                .font(theme::semibold(12.0))
                .color(palette.warning)
                .extra_letter_spacing(0.6),
        );
        ui.add_space(4.0);
        let mut first = true;
        let mut gap = |ui: &mut egui::Ui| {
            if !std::mem::take(&mut first) {
                ui.add_space(8.0);
            }
        };
        if let Some(hash) = file {
            gap(ui);
            file_changed_items(app, ui, entry, hash, actions);
        }
        if !pending.is_empty() {
            gap(ui);
            pending_items(app, ui, entry, pending, actions);
        }
        if missing {
            gap(ui);
            ui.add(
                egui::Label::new(
                    egui::RichText::new("Fill in the settings marked Required.")
                        .font(theme::regular(13.0))
                        .color(palette.text),
                )
                .wrap(),
            );
        }
    });
}

/// Chaat's status line, the exclusive-capability note and cross-links.
fn status_section(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    state: Option<&PluginState>,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    group(ui, &palette, "Status");
    let stopped;
    let (text, color) = if !entry.enabled {
        ("Off", palette.dim)
    } else if state.is_some_and(|s| s.running) {
        ("Running", palette.success)
    } else if let Some(message) = state.and_then(|s| s.message.as_deref()) {
        stopped = format!("Stopped: {message}");
        (stopped.as_str(), palette.danger)
    } else {
        ("Starting\u{2026}", palette.dim)
    };
    ui.horizontal(|ui| {
        let (rect, _) = ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
        ui.painter().circle_filled(rect.center(), 4.0, color);
        ui.add(
            egui::Label::new(
                egui::RichText::new(text)
                    .font(theme::regular(13.0))
                    .color(palette.secondary),
            )
            .wrap(),
        );
    });
    let offers_alerts = state.is_some_and(|s| s.offers.contains(&Capability::CallAlerts));
    let owner = app.call_alerts_owner();
    if offers_alerts && entry.enabled && owner.as_deref().is_some_and(|o| o != entry.id) {
        note(
            ui,
            &palette,
            "Another plugin provides call alerts, so this one's rings are ignored.",
        );
    }
    for capability in app.plugin_provides(&entry.id) {
        let (line, label, page) = if capability == Capability::Push {
            (
                "Push updates use this plugin's tunnel.",
                "Open Push updates \u{203A}",
                SettingsPage::Push,
            )
        } else {
            (
                "Call alerts come from this plugin.",
                "Open Calls \u{203A}",
                SettingsPage::Calls,
            )
        };
        ui.horizontal_wrapped(|ui| {
            note(ui, &palette, line);
            if widgets::text_button(ui, &palette, label, theme::regular(13.0)).clicked() {
                actions.push(Action::ShowSettingsPage(page));
            }
        });
    }
}

/// A bullet per item the plugin gets, in the words of the consent question.
fn gets_from_chaat(app: &App, ui: &mut egui::Ui, entry: &PluginEntry) {
    let palette = app.palette;
    group(ui, &palette, "Gets from Chaat");
    let receives = app.plugin_receives(&entry.id);
    if receives.is_empty() {
        bullet(ui, &palette, "Nothing");
    }
    for need in receives {
        bullet(ui, &palette, need_words(need));
    }
}

fn bullet(ui: &mut egui::Ui, palette: &theme::Palette, text: &str) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(format!("\u{2022} {text}"))
                .font(theme::regular(13.0))
                .color(palette.secondary),
        )
        .wrap(),
    );
}

/// The plugin's options in one `surface` card under a caption that marks the
/// text as the plugin's.
fn settings_card(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    state: &PluginState,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    group(ui, &palette, "Settings \u{00B7} from the plugin");
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(palette.surface)
        .corner_radius(8)
        .inner_margin(egui::Margin::symmetric(12, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            let cx = OptionsCx::new(app, entry, state);
            let all: Vec<&OptionSpec> = state.options.iter().collect();
            options_block(&cx, ui, &all, actions);
        });
}

/// Path, pin state and Remove, folded by default.
fn file_section(app: &App, ui: &mut egui::Ui, entry: &PluginEntry, actions: &mut Vec<Action>) {
    let palette = app.palette;
    ui.add_space(14.0);
    egui::CollapsingHeader::new(
        egui::RichText::new("FILE")
            .font(theme::semibold(12.0))
            .color(palette.dim)
            .extra_letter_spacing(0.6),
    )
    .id_salt(("plugin-file", &entry.id))
    .show(ui, |ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(entry.path.display().to_string())
                    .font(theme::regular(12.0))
                    .color(palette.dim),
            )
            .wrap(),
        );
        let pin = if entry.sha256.is_some() {
            "Checked against the file you added."
        } else {
            "Not checked yet."
        };
        note(ui, &palette, pin);
        ui.add_space(6.0);
        let confirming = ui
            .data(|d| d.get_temp::<bool>(confirm_id(&entry.id)))
            .unwrap_or(false);
        if !confirming {
            if widgets::secondary_button(ui, &palette, "Remove plugin\u{2026}").clicked() {
                set_confirming(ui, &entry.id, true);
            }
            return;
        }
        let mut ask = format!(
            "Remove {}? Chaat stops it, deletes its saved secrets and tells it to forget your sign-ins.",
            app.quoted_plugin_name(&entry.id)
        );
        // A provider also says what stops or who takes over.
        if let Some(consequences) = app.turn_off_consequences(&entry.id) {
            ask.push(' ');
            ask.push_str(&consequences);
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(ask)
                .font(theme::regular(13.0))
                .color(palette.text),
            )
            .wrap(),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if danger_button(ui, &palette, "Remove").clicked() {
                set_confirming(ui, &entry.id, false);
                actions.push(Action::Plugin(PluginAction::Remove(entry.id.clone())));
            }
            if widgets::secondary_button(ui, &palette, "Cancel").clicked() {
                set_confirming(ui, &entry.id, false);
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
    let narrow = ui.available_width() < 520.0;
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
        let (text, color) = if listening {
            ("Listening for calls", palette.success)
        } else if waiting {
            ("Waiting for sign-in", palette.warning)
        } else {
            ("Connecting\u{2026}", palette.dim)
        };
        // Narrow: the status goes under the name.
        let content = |ui: &mut egui::Ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&name)
                        .font(theme::regular(13.0))
                        .color(palette.text),
                )
                .truncate(),
            );
            ui.horizontal_wrapped(|ui| {
                ui.label(
                    egui::RichText::new(text)
                        .font(theme::regular(12.0))
                        .color(color),
                );
                if waiting && widgets::secondary_button(ui, &palette, "New code").clicked() {
                    actions.push(Action::Plugin(PluginAction::SignIn {
                        plugin: entry.id.clone(),
                        account: account.id.clone(),
                    }));
                }
            });
        };
        if narrow {
            ui.vertical(content);
        } else {
            ui.horizontal(content);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn link_host_is_empty_for_a_missing_host() {
        assert_eq!(link_host("https://:443/x"), None);
        assert_eq!(
            link_host("https://example.com:443/x").as_deref(),
            Some("example.com")
        );
    }
}
