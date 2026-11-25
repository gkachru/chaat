//! Teams and their channels, as a collapsible tree.

use egui::{CornerRadius, Sense, vec2};

use super::widgets;
use crate::app::App;
use crate::model::{Action, ChannelKind, ConversationId};
use crate::theme::{self, Icon};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let unified = app.unified_view();
    let indexes: Vec<usize> = if unified {
        (0..app.accounts.len()).collect()
    } else {
        vec![app.active]
    };
    if indexes.iter().all(|&j| app.accounts[j].teams.is_empty()) {
        let (title, text) = if app.loading.contains(&crate::sync::Resource::Teams) {
            (
                "Loading teams…",
                "Your teams appear here as soon as Microsoft answers.",
            )
        } else {
            ("No teams", "Teams you're a member of show up here.")
        };
        widgets::empty_state(ui, &palette, Icon::Teams, title, text);
        return;
    }
    let active = app.active;
    let mut actions = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt("teams")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 1.0;
            for &j in &indexes {
                let account = &app.accounts[j];
                if account.teams.is_empty() {
                    continue;
                }
                if unified {
                    account_header(ui, app, j);
                }
                for team in &account.teams {
                    let expanded = account.expanded_teams.contains(&team.id);
                    let response = team_row(ui, app, &team.name, expanded);
                    if response.clicked() {
                        let toggle = Action::ToggleTeam(team.id.clone());
                        actions.push(if j == active {
                            toggle
                        } else {
                            Action::ForAccount(j, Box::new(toggle))
                        });
                    }
                    if !expanded {
                        continue;
                    }
                    match account.channels.get(&team.id) {
                        Some(channels) if !channels.is_empty() => {
                            for channel in channels {
                                let id = ConversationId::Channel {
                                    team: team.id.clone(),
                                    channel: channel.id.clone(),
                                };
                                let selected = j == active && app.open.as_ref() == Some(&id);
                                let response =
                                    channel_row(ui, app, &channel.name, channel.kind, selected);
                                if response.clicked() {
                                    if j != active {
                                        actions.push(Action::SwitchAccount(j));
                                    }
                                    actions.push(Action::OpenConversation(id));
                                }
                            }
                        }
                        _ => {
                            ui.horizontal(|ui| {
                                ui.add_space(52.0);
                                ui.label(
                                    egui::RichText::new("Loading channels…")
                                        .color(palette.dim)
                                        .font(theme::regular(12.0)),
                                );
                            });
                        }
                    }
                    ui.add_space(6.0);
                }
            }
            ui.add_space(12.0);
        });
    app.actions.extend(actions);
}

/// Unified: the account's mark and name above its teams.
fn account_header(ui: &mut egui::Ui, app: &App, j: usize) {
    let look = app.account_look(j);
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::hover());
    let mark = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 16.0, rect.center().y - 9.0),
        egui::Vec2::splat(18.0),
    );
    widgets::paint_account_mark(ui, &look, mark);
    let galley = widgets::elided(
        ui,
        &look.name,
        theme::semibold(12.0),
        app.palette.secondary,
        rect.right() - mark.right() - 24.0,
    );
    ui.painter().galley(
        egui::pos2(mark.right() + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        app.palette.secondary,
    );
}

fn team_row(ui: &mut egui::Ui, app: &App, name: &str, expanded: bool) -> egui::Response {
    let palette = &app.palette;
    let (response, inner) = widgets::list_row(ui, palette, 40.0, false, Sense::click());
    if ui.is_rect_visible(response.rect) {
        let chevron = if expanded {
            Icon::ChevronDown
        } else {
            Icon::ChevronRight
        };
        chevron.image(palette.dim, 14.0).paint_at(
            ui,
            egui::Rect::from_center_size(
                egui::pos2(inner.left() + 12.0, inner.center().y),
                egui::Vec2::splat(14.0),
            ),
        );
        let tile = egui::Rect::from_center_size(
            egui::pos2(inner.left() + 38.0, inner.center().y),
            egui::Vec2::splat(26.0),
        );
        ui.painter().rect_filled(
            tile,
            CornerRadius::same(7),
            theme::avatar_color(name, palette.dark),
        );
        ui.painter().text(
            tile.center(),
            egui::Align2::CENTER_CENTER,
            theme::initials(name),
            theme::bold(11.0),
            egui::Color32::WHITE,
        );
        let galley = super::widgets::elided(
            ui,
            name,
            theme::semibold(14.0),
            palette.text,
            inner.right() - tile.right() - 16.0,
        );
        ui.painter().galley(
            egui::pos2(
                tile.right() + 10.0,
                inner.center().y - galley.size().y / 2.0,
            ),
            galley,
            palette.text,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn channel_row(
    ui: &mut egui::Ui,
    app: &App,
    name: &str,
    kind: ChannelKind,
    selected: bool,
) -> egui::Response {
    let palette = &app.palette;
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 32.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let inner = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 36.0, rect.top() + 1.0),
            egui::pos2(rect.right() - 8.0, rect.bottom() - 1.0),
        );
        if selected || response.hovered() {
            ui.painter().rect_filled(
                inner,
                CornerRadius::same(7),
                if selected {
                    palette.surface_active
                } else {
                    palette.surface_hover
                },
            );
        }
        let icon = match kind {
            ChannelKind::Private => Icon::Lock,
            ChannelKind::Shared => Icon::Teams,
            ChannelKind::Standard => Icon::Hash,
        };
        let tint = if selected {
            palette.accent
        } else {
            palette.dim
        };
        icon.image(tint, 14.0).paint_at(
            ui,
            egui::Rect::from_center_size(
                egui::pos2(inner.left() + 16.0, inner.center().y),
                egui::Vec2::splat(14.0),
            ),
        );
        let galley = super::widgets::elided(
            ui,
            name,
            if selected {
                theme::semibold(13.0)
            } else {
                theme::regular(13.0)
            },
            if selected {
                palette.text
            } else {
                palette.secondary
            },
            inner.width() - 40.0,
        );
        ui.painter().galley(
            egui::pos2(
                inner.left() + 32.0,
                inner.center().y - galley.size().y / 2.0,
            ),
            galley,
            palette.text,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}
