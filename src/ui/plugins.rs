//! Settings › Plugins: the plugins Chaat launches. The words about what a
//! plugin is are Chaat's; the plugin's own name and description show apart.

use super::settings::{group, note};
use super::widgets::{self, PointerControls as _};
use crate::app::App;
use crate::model::Action;
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
    for entry in &app.settings.plugins {
        let state = app.plugins.get(&entry.id);
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(app.plugin_name(&entry.id))
                        .font(theme::medium(13.0))
                        .color(palette.text),
                )
                .truncate(),
            );
        });
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
        ui.add(
            egui::Label::new(
                egui::RichText::new(entry.path.display().to_string())
                    .font(theme::regular(12.0))
                    .color(palette.dim),
            )
            .truncate(),
        );
        let (status, color) = if !entry.enabled {
            ("Off", palette.dim)
        } else if state.is_some_and(|s| s.running) {
            ("Running", palette.secondary)
        } else if let Some(message) = state.and_then(|s| s.message.as_deref()) {
            (message, palette.warning)
        } else {
            ("Not running", palette.warning)
        };
        ui.label(
            egui::RichText::new(status)
                .font(theme::regular(13.0))
                .color(color),
        );
        ui.horizontal(|ui| {
            let mut enabled = entry.enabled;
            if ui.checkbox_hand(&mut enabled, "Turn on").changed() {
                actions.push(Action::SetPluginEnabled(entry.id.clone(), enabled));
            }
            if widgets::secondary_button(ui, &palette, "Remove").clicked() {
                actions.push(Action::RemovePlugin(entry.id.clone()));
            }
        });
    }
    ui.add_space(10.0);
    if widgets::secondary_button(ui, &palette, "Add plugin\u{2026}").clicked() {
        actions.push(Action::AddPlugin);
    }
}
