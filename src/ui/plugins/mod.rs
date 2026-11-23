//! Settings › Plugins: the plugins Chaat launches. The words about what a
//! plugin is are Chaat's; the plugin's own name and description show apart.

mod list;
mod options;
mod page;
mod setup;

use super::widgets;
use crate::app::{App, PluginsView};
use crate::model::{Action, PluginAction};
use crate::settings::PluginEntry;
use crate::theme;
use options::drop_stale_secret_buffers;

pub use options::forget_secret_buffers;

pub fn page(app: &App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    drop_stale_secret_buffers(app, ui);
    drop_stale_questions(app, ui);
    match &app.plugins_view {
        PluginsView::Adding(id) => {
            if let Some(entry) = app.settings.plugins.iter().find(|e| &e.id == id) {
                setup::setup_view(app, ui, entry, actions);
                return;
            }
        }
        PluginsView::Plugin(id) => {
            if let Some(entry) = app.settings.plugins.iter().find(|e| &e.id == id) {
                page::plugin_page(app, ui, entry, actions);
                return;
            }
        }
        PluginsView::List => {}
    }
    list::list_view(app, ui, actions);
}

/// Chaat's words for what one kind of data is.
fn need_words(need: &str) -> &'static str {
    match need {
        "accounts" => "your accounts' IDs and sign-in names",
        "presence" => "your presence (availability and activity)",
        "options" => "its own settings below",
        "push" => "your update notifications: which chats changed and when, no message text",
        "messages" => "the text of messages in the chats and channels you allow",
        _ => "something Chaat does not know",
    }
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

/// The question about pending needs, with its Allow and Don't allow buttons.
fn pending_items(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    pending: &[&'static str],
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    {
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
                actions.push(Action::Plugin(PluginAction::ApproveNeeds(entry.id.clone())));
            }
            if widgets::secondary_button(ui, &palette, "Don't allow").clicked() {
                actions.push(Action::Plugin(PluginAction::DenyNeeds(entry.id.clone())));
            }
        });
    }
}

/// The file-changed question with its two buttons.
fn file_changed_items(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    hash: &str,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    {
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "The plugin file changed since you added it. Run the new file only if you trust it. It gets the settings and secrets you saved for this plugin, and asks again for anything else.",
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
                actions.push(Action::Plugin(PluginAction::TrustFile(entry.id.clone())));
            }
            if widgets::secondary_button(ui, &palette, "Keep it off").clicked() {
                actions.push(Action::Plugin(PluginAction::KeepOff(entry.id.clone())));
            }
        });
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

/// Forgets the Remove confirmation and the turn-off question when the view
/// changed (a plugin's page, the list) or the page was not drawn the frame
/// before (Settings was closed): they must not wait for the next visit.
fn drop_stale_questions(app: &App, ui: &egui::Ui) {
    let key = egui::Id::new("plugins-view-seen");
    let frame = ui.ctx().cumulative_frame_nr();
    let now = format!("{:?}", app.plugins_view);
    let same = ui
        .data(|d| d.get_temp::<(u64, String)>(key))
        .is_some_and(|(last, view)| view == now && frame <= last.saturating_add(1));
    if !same {
        for entry in &app.settings.plugins {
            page::forget_questions(ui, &entry.id);
        }
    }
    ui.data_mut(|d| d.insert_temp(key, (frame, now)));
}
