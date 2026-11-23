//! The add flow of a new plugin: Starting, Setup, Failed. Chaat's words
//! frame the page; the plugin's own name and description show apart.

use super::super::widgets::{self, PointerControls as _};
use super::super::widgets::{group, note};
use super::options::{OptionsCx, option_row, options_block};
use super::{heading, need_words};
use crate::app::{App, SummaryLevel};
use crate::model::{Action, PluginAction};
use crate::plugin::protocol::OptionSpec;
use crate::settings::PluginEntry;
use crate::theme;
use std::time::Duration;

/// Where the add flow stands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    Starting,
    Setup,
    Failed,
}

fn stage(app: &App, entry: &PluginEntry) -> Stage {
    let state = app.plugins.get(&entry.id);
    let running = state.is_some_and(|s| s.running);
    if app.plugin_summary(&entry.id).level == SummaryLevel::Error
        || (!running && state.is_some_and(|s| s.message.is_some()))
    {
        Stage::Failed
    } else if app.plugin_ready(&entry.id) {
        Stage::Setup
    } else {
        Stage::Starting
    }
}

fn ticked_id(plugin: &str) -> egui::Id {
    egui::Id::new(("plugin-setup-ticked", plugin))
}

fn ticked(ui: &egui::Ui, plugin: &str) -> Vec<String> {
    ui.data(|d| d.get_temp::<Vec<String>>(ticked_id(plugin)))
        .unwrap_or_default()
}

/// Forgets the scratch of one add flow.
fn clear_scratch(ui: &egui::Ui, plugin: &str) {
    ui.data_mut(|d| {
        d.remove::<Vec<String>>(ticked_id(plugin));
    });
}

pub(super) fn setup_view(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    ui.add(
        egui::Label::new(
            egui::RichText::new("Set up a new plugin")
                .font(theme::bold(20.0))
                .color(palette.text),
        )
        .wrap(),
    );
    ui.add_space(6.0);
    heading(app, ui, entry);
    match stage(app, entry) {
        Stage::Starting => starting(app, ui, entry, actions),
        Stage::Setup => setup(app, ui, entry, actions),
        Stage::Failed => failed(app, ui, entry, actions),
    }
}

fn starting(app: &App, ui: &mut egui::Ui, entry: &PluginEntry, actions: &mut Vec<Action>) {
    let palette = app.palette;
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        widgets::loading(ui, palette.dim, 16.0);
        ui.label(
            egui::RichText::new("Starting the plugin\u{2026}")
                .font(theme::regular(13.0))
                .color(palette.secondary),
        );
    });
    // One repaint at the time limit, so the failure shows without polling.
    // Past the limit the stage is Failed and nothing is scheduled.
    if let Some(remaining) = app.plugin_start_remaining(&entry.id)
        && !remaining.is_zero()
    {
        ui.ctx()
            .request_repaint_after(remaining + Duration::from_millis(50));
    }
    ui.add_space(10.0);
    if widgets::secondary_button(ui, &palette, "Cancel").clicked() {
        clear_scratch(ui, &entry.id);
        actions.push(Action::Plugin(PluginAction::CancelSetup(entry.id.clone())));
    }
}

fn failed(app: &App, ui: &mut egui::Ui, entry: &PluginEntry, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let summary = app.plugin_summary(&entry.id);
    // Chaat's own stop reason (from the host) when it has one.
    let stop = app
        .plugins
        .get(&entry.id)
        .and_then(|s| s.message.clone())
        .filter(|_| !app.plugin_ready(&entry.id));
    let reason = match stop {
        Some(message) => format!("Stopped: {message}"),
        None if summary.level == SummaryLevel::Error => summary.text,
        None => "It stopped before saying what it does".to_owned(),
    };
    ui.add_space(6.0);
    ui.add(
        egui::Label::new(
            egui::RichText::new(reason)
                .font(theme::regular(13.0))
                .color(palette.danger),
        )
        .wrap(),
    );
    note(
        ui,
        &palette,
        "Check that the file is a Chaat plugin, then try again. Remove deletes nothing on disk.",
    );
    ui.add_space(10.0);
    ui.horizontal(|ui| {
        if widgets::secondary_button(ui, &palette, "Remove").clicked() {
            clear_scratch(ui, &entry.id);
            actions.push(Action::Plugin(PluginAction::CancelSetup(entry.id.clone())));
        }
        if widgets::primary_button(ui, &palette, "Try again").clicked() {
            actions.push(Action::Plugin(PluginAction::RetryStart(entry.id.clone())));
        }
    });
}

fn setup(app: &App, ui: &mut egui::Ui, entry: &PluginEntry, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let state = app.plugins.get(&entry.id);
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
            egui::RichText::new(format!("From {}", entry.path.display()))
                .font(theme::regular(12.0))
                .color(palette.dim),
        )
        .wrap(),
    );
    if let Some(state) = state {
        if !state.pending_needs.is_empty() {
            group(ui, &palette, "It asks for");
            let mut now = ticked(ui, &entry.id);
            for need in &state.pending_needs {
                let mut on = now.iter().any(|n| n == need);
                if ui.checkbox_hand(&mut on, need_words(need)).changed() {
                    now.retain(|n| n != need);
                    if on {
                        now.push((*need).to_owned());
                    }
                    ui.data_mut(|d| d.insert_temp(ticked_id(&entry.id), now.clone()));
                }
            }
            note(ui, &palette, "(nothing is sent until you tick it)");
        }
        let cx = OptionsCx::new(app, entry, state);
        let required: Vec<&OptionSpec> = state
            .options
            .iter()
            .filter(|spec| spec.required && cx.visible(spec))
            .collect();
        let rest: Vec<&OptionSpec> = state.options.iter().filter(|spec| !spec.required).collect();
        if !required.is_empty() || rest.iter().any(|spec| cx.visible(spec)) {
            // The plugin's own words sit under a caption that says so.
            group(ui, &palette, "Settings \u{00B7} from the plugin");
        }
        if !required.is_empty() {
            group(ui, &palette, "Required");
            for spec in required {
                option_row(&cx, ui, spec, actions);
            }
        }
        if rest.iter().any(|spec| cx.visible(spec)) {
            group(ui, &palette, "More settings");
            options_block(&cx, ui, &rest, actions);
        }
    }
    ui.add_space(10.0);
    let can_finish = app.plugin_missing_required(&entry.id).is_empty();
    ui.horizontal(|ui| {
        if widgets::secondary_button(ui, &palette, "Cancel").clicked() {
            clear_scratch(ui, &entry.id);
            actions.push(Action::Plugin(PluginAction::CancelSetup(entry.id.clone())));
        }
        ui.add_enabled_ui(can_finish, |ui| {
            if widgets::primary_button(ui, &palette, "Done").clicked() {
                let on = ticked(ui, &entry.id);
                let allow: Vec<String> = state
                    .map(|s| s.pending_needs.clone())
                    .unwrap_or_default()
                    .into_iter()
                    .filter(|need| on.iter().any(|n| n == need))
                    .map(str::to_owned)
                    .collect();
                clear_scratch(ui, &entry.id);
                actions.push(Action::Plugin(PluginAction::FinishSetup {
                    id: entry.id.clone(),
                    allow,
                }));
            }
        });
    });
}
