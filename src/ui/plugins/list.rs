//! The plugins list: one row per plugin with its state at a glance. Every
//! word here is Chaat's, except the plugin's cleaned name (and, inside the
//! summary, a tunnel host Chaat already checked). No plugin status message
//! is shown.

use super::super::settings::{group, note};
use super::super::widgets::{self, PointerControls as _};
use super::page::{forget_confirm, request_enabled, turn_off_ask};
use crate::app::{App, PluginSummary, SummaryLevel};
use crate::model::Action;
use crate::plugin::registry;
use crate::theme::{self, Icon, Palette};
use egui::{Align, Frame, Layout, Margin, Sense, Stroke, UiBuilder, vec2};

/// Below this width the header button says "Add…".
const NARROW: f32 = 520.0;

pub(super) fn list_view(app: &App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let narrow = ui.available_width() < NARROW;
    // A confirmation left open on a plugin page does not outlive the page.
    for entry in &app.settings.plugins {
        forget_confirm(ui, &entry.id);
    }
    let reordering = app.reordering && app.settings.plugins.len() >= 2;
    // The buttons keep their place in the header whatever the list holds.
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if reordering {
                if widgets::primary_button(ui, &palette, "Done").clicked() {
                    actions.push(Action::EndReorder);
                }
                return;
            }
            let label = if narrow {
                "Add\u{2026}"
            } else {
                "Add plugin\u{2026}"
            };
            if widgets::secondary_button(ui, &palette, label).clicked() {
                actions.push(Action::AddPlugin);
            }
            if app.settings.plugins.len() >= 2
                && widgets::secondary_button(ui, &palette, "Reorder").clicked()
            {
                actions.push(Action::StartReorder);
            }
        });
    });
    if reordering {
        note(
            ui,
            &palette,
            "The first plugin that offers push or call alerts provides it.",
        );
        ui.add_space(8.0);
        reorder_card(app, ui, actions);
        return;
    }
    // Chaat's own words, whatever a plugin says about itself.
    note(
        ui,
        &palette,
        "Separate programs you add. Chaat sends each one only what you allow.",
    );
    if app.settings.plugins.is_empty() {
        widgets::empty_state(
            ui,
            &palette,
            Icon::Settings,
            "No plugins yet",
            "Add a plugin you trust to extend Chaat, such as call alerts or a status light.",
        );
        ui.add_space(10.0);
        ui.vertical_centered(|ui| {
            if widgets::secondary_button(ui, &palette, "Add plugin\u{2026}").clicked() {
                actions.push(Action::AddPlugin);
            }
        });
        return;
    }
    let rows: Vec<(String, PluginSummary)> = app
        .settings
        .plugins
        .iter()
        .map(|entry| (entry.id.clone(), app.plugin_summary(&entry.id)))
        .collect();
    let (needs, rest): (Vec<_>, Vec<_>) = rows.iter().partition(|(_, s)| s.needs_you());
    if !needs.is_empty() {
        group(ui, &palette, "Needs you");
        card(app, ui, &needs, actions);
    }
    if !rest.is_empty() {
        if !needs.is_empty() {
            ui.add_space(8.0);
        }
        card(app, ui, &rest, actions);
    }
}

/// One `surface` card holding rows split by `outline` lines.
fn card(
    app: &App,
    ui: &mut egui::Ui,
    rows: &[&(String, PluginSummary)],
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    Frame::new()
        .fill(palette.surface)
        .corner_radius(8)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (index, (id, summary)) in rows.iter().enumerate() {
                if index > 0 {
                    let y = ui.cursor().top();
                    let rect = ui.max_rect();
                    ui.painter()
                        .hline(rect.x_range(), y, Stroke::new(1.0, palette.outline));
                }
                row(app, ui, id, summary, actions);
                turn_off_ask(app, ui, id, actions);
            }
        });
}

fn dot_color(palette: &Palette, level: SummaryLevel) -> egui::Color32 {
    match level {
        SummaryLevel::Attention => palette.warning,
        SummaryLevel::Error => palette.danger,
        SummaryLevel::Ok => palette.success,
        SummaryLevel::Off => palette.dim,
    }
}

/// One plugin: status dot, name, summary, the on/off switch and a chevron.
/// A click anywhere but the switch opens the plugin.
fn row(app: &App, ui: &mut egui::Ui, id: &str, summary: &PluginSummary, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let name = app.plugin_name(id);
    let enabled = app
        .settings
        .plugins
        .iter()
        .find(|e| e.id == id)
        .is_some_and(|e| e.enabled);
    let row = ui
        .scope_builder(UiBuilder::new().sense(Sense::click()), |ui| {
            Frame::new()
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let (chevron, _) =
                                ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                            Icon::ChevronRight
                                .image(palette.dim, 14.0)
                                .paint_at(ui, chevron);
                            let mut on = enabled;
                            if ui.checkbox_hand(&mut on, "Turn on").changed() {
                                request_enabled(app, ui, id, on, actions);
                            }
                            ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                let (rect, _) =
                                    ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                                let color = dot_color(&palette, summary.level);
                                if enabled || summary.level != SummaryLevel::Off {
                                    ui.painter().circle_filled(rect.center(), 4.0, color);
                                } else {
                                    // A hollow dot for a plugin that is off.
                                    ui.painter().circle_stroke(
                                        rect.center(),
                                        3.5,
                                        Stroke::new(1.5, color),
                                    );
                                }
                                ui.vertical(|ui| {
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&name)
                                                .font(theme::semibold(15.0))
                                                .color(palette.text),
                                        )
                                        .truncate()
                                        .selectable(false),
                                    )
                                    .on_hover_text(&name);
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(&summary.text)
                                                .font(theme::regular(13.0))
                                                .color(palette.secondary),
                                        )
                                        .truncate()
                                        .selectable(false),
                                    )
                                    .on_hover_text(&summary.text);
                                });
                            });
                        });
                    });
                });
        })
        .response;
    widgets::hover_wash(ui, &palette, &row, 0);
    let row = row.on_hover_cursor(egui::CursorIcon::PointingHand);
    if row.clicked() {
        actions.push(Action::OpenPlugin(id.to_owned()));
    }
}

/// What a plugin does for push and call alerts, for the reorder mode.
fn reorder_summary(app: &App, id: &str) -> String {
    let provides = app.plugin_provides(id);
    let word = |c: &str| {
        if c == registry::PUSH {
            "push updates"
        } else {
            "call alerts"
        }
    };
    if !provides.is_empty() {
        let words: Vec<&str> = provides.iter().map(|c| word(c)).collect();
        return format!("Provides {}", words.join(" and "));
    }
    let running = app.plugins.get(id).filter(|s| s.running);
    let enabled = app.settings.plugins.iter().any(|e| e.id == id && e.enabled);
    let mut beaten = Vec::new();
    if let (true, Some(state)) = (enabled, running) {
        // A relay address means no plugin provides push.
        let push_owner = if app.settings.relay_url.is_none() {
            app.push_owner()
        } else {
            None
        };
        for (capability, owner) in [
            (registry::PUSH, push_owner),
            (registry::CALL_ALERTS, app.call_alerts_owner()),
        ] {
            if state.offers.contains(&capability) && owner.is_some_and(|o| o != id) {
                beaten.push(word(capability));
            }
        }
    }
    if beaten.is_empty() {
        String::new()
    } else {
        format!(
            "Offers {}; the one above provides them",
            beaten.join(" and ")
        )
    }
}

/// Every plugin in its true order, with a drag handle and move buttons.
fn reorder_card(app: &App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let ids: Vec<String> = app.settings.plugins.iter().map(|e| e.id.clone()).collect();
    let last = ids.len().saturating_sub(1);
    let mut rects: Vec<egui::Rect> = Vec::new();
    let mut dropped: Option<usize> = None;
    let mut dragging: Option<usize> = None;
    Frame::new()
        .fill(palette.surface)
        .corner_radius(8)
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (index, id) in ids.iter().enumerate() {
                if index > 0 {
                    let y = ui.cursor().top();
                    let rect = ui.max_rect();
                    ui.painter()
                        .hline(rect.x_range(), y, Stroke::new(1.0, palette.outline));
                }
                let name = app.plugin_name(id);
                let summary = reorder_summary(app, id);
                let row = Frame::new()
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            let handle = grip(ui, &palette);
                            if handle.dragged() {
                                dragging = Some(index);
                            }
                            if handle.drag_stopped() {
                                dropped = Some(index);
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if index < last && move_button(ui, &palette, "\u{2193}") {
                                    actions.push(Action::MovePlugin {
                                        id: id.clone(),
                                        to: index + 1,
                                    });
                                }
                                if index > 0 && move_button(ui, &palette, "\u{2191}") {
                                    actions.push(Action::MovePlugin {
                                        id: id.clone(),
                                        to: index - 1,
                                    });
                                }
                                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                                    ui.vertical(|ui| {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(&name)
                                                    .font(theme::semibold(15.0))
                                                    .color(palette.text),
                                            )
                                            .truncate()
                                            .selectable(false),
                                        )
                                        .on_hover_text(&name);
                                        if !summary.is_empty() {
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(&summary)
                                                        .font(theme::regular(13.0))
                                                        .color(palette.secondary),
                                                )
                                                .wrap()
                                                .selectable(false),
                                            );
                                        }
                                    });
                                });
                            });
                        });
                    });
                rects.push(row.response.rect);
            }
        });
    // The row under the pointer is where a dragged one lands.
    let pointer_y = ui.input(|i| i.pointer.latest_pos().map(|p| p.y));
    let target = pointer_y.map(|y| {
        rects
            .iter()
            .position(|r| y < r.bottom())
            .unwrap_or(rects.len().saturating_sub(1))
    });
    if let (Some(from), Some(to)) = (dragging, target)
        && from != to
        && let Some(rect) = rects.get(to)
    {
        let y = if to < from { rect.top() } else { rect.bottom() };
        ui.painter()
            .hline(rect.x_range(), y, Stroke::new(2.0, palette.accent));
    }
    if let (Some(from), Some(to)) = (dropped, target)
        && from != to
    {
        actions.push(Action::MovePlugin {
            id: ids[from].clone(),
            to,
        });
    }
}

/// Six dots the person drags a row by.
fn grip(ui: &mut egui::Ui, palette: &Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(16.0, 28.0), Sense::drag());
    let response = response
        .on_hover_cursor(egui::CursorIcon::Grab)
        .on_hover_text("Drag to move");
    let color = if response.hovered() || response.dragged() {
        palette.text
    } else {
        palette.dim
    };
    for col in [-3.0, 3.0] {
        for row in [-6.0, 0.0, 6.0] {
            ui.painter()
                .circle_filled(rect.center() + vec2(col, row), 1.5, color);
        }
    }
    response
}

/// A small square arrow button; the text is the arrow.
fn move_button(ui: &mut egui::Ui, palette: &Palette, arrow: &str) -> bool {
    let hint = if arrow == "\u{2191}" {
        "Move up"
    } else {
        "Move down"
    };
    let button = egui::Button::new(
        egui::RichText::new(arrow)
            .font(theme::medium(14.0))
            .color(palette.text),
    )
    .fill(palette.surface)
    .stroke(Stroke::new(1.0, palette.outline))
    .corner_radius(8)
    .min_size(vec2(32.0, 32.0));
    ui.add(button)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(hint)
        .clicked()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{Backend, Waker};
    use crate::paths::AppDirs;
    use crate::settings::{PluginEntry, Settings};

    #[test]
    fn reorder_summary_ignores_push_with_a_relay() {
        let (backend, _) = Backend::recording();
        let dir = std::env::temp_dir().join("chaat-test-unused");
        let mut app = App::with_backend(
            AppDirs::under(&dir),
            Settings::default(),
            Waker::default(),
            backend,
            false,
        );
        for id in ["a", "b"] {
            app.settings.plugins.push(PluginEntry {
                id: id.into(),
                enabled: true,
                ..PluginEntry::default()
            });
            let state = app.plugins.entry(id.into()).or_default();
            state.running = true;
            state.offers = vec![registry::PUSH];
        }
        assert_eq!(reorder_summary(&app, "a"), "Provides push updates");
        assert_eq!(
            reorder_summary(&app, "b"),
            "Offers push updates; the one above provides them"
        );
        app.settings.relay_url = Some("https://relay.example".into());
        assert_eq!(reorder_summary(&app, "a"), "");
        assert_eq!(reorder_summary(&app, "b"), "");
    }
}
