//! The avatar's menu: availability, status message, out of office and
//! notification pauses. Accounts live in the rail's tiles.

use egui::{CornerRadius, Sense, vec2};

use crate::app::App;
use crate::model::{Action, Availability};
use crate::notify::schedule::PauseFor;
use crate::theme::{self, Icon};

/// The avatar's menu: availability and status message. Settings has its own
/// button in the rail.
pub(super) fn account_menu(app: &mut App, avatar: &egui::Response) {
    let palette = app.palette;
    let mut actions = Vec::new();
    egui::Popup::menu(avatar)
        .align(egui::RectAlign::RIGHT_END)
        .gap(8.0)
        .width(280.0)
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            // Availability.
            let current = app.my_presence.as_ref().map(|p| p.availability);
            for (availability, label) in [
                (Some(Availability::Available), "Available"),
                (Some(Availability::Busy), "Busy"),
                (Some(Availability::DoNotDisturb), "Do not disturb"),
                (Some(Availability::BeRightBack), "Be right back"),
                (Some(Availability::Away), "Appear away"),
                (Some(Availability::Offline), "Appear offline"),
                (None, "Reset status"),
            ] {
                let response =
                    menu_row(
                        ui,
                        &palette,
                        label,
                        palette.text,
                        |ui, at| match availability {
                            Some(availability) => {
                                ui.painter()
                                    .circle_filled(at, 5.0, palette.presence(availability));
                            }
                            None => paint_icon(ui, Icon::Refresh, at, palette.dim),
                        },
                    );
                let rect = response.rect;
                if availability.is_some() && availability == current {
                    Icon::Check.image(palette.accent, 14.0).paint_at(
                        ui,
                        egui::Rect::from_center_size(
                            egui::pos2(rect.right() - 14.0, rect.center().y),
                            egui::Vec2::splat(14.0),
                        ),
                    );
                }
                if response.clicked() {
                    actions.push(Action::SetAvailability(availability));
                    ui.close();
                }
            }
            let message = app
                .my_presence
                .as_ref()
                .and_then(|p| p.status_message.clone())
                .unwrap_or_else(|| "Set a status message".into());
            let status = menu_row(
                ui,
                &palette,
                &crate::html::truncate(&message, 34),
                palette.secondary,
                |ui, at| paint_icon(ui, Icon::Pencil, at, palette.secondary),
            );
            if status.clicked() {
                actions.push(Action::OpenStatusMessage);
                ui.close();
            }
            let away = app
                .out_of_office
                .as_ref()
                .is_some_and(|o| o.active(crate::time::now()));
            let out_of_office = menu_row(
                ui,
                &palette,
                if away {
                    "Out of office: on"
                } else {
                    "Out of office…"
                },
                if away {
                    palette.text
                } else {
                    palette.secondary
                },
                |ui, at| {
                    paint_icon(
                        ui,
                        Icon::Calendar,
                        at,
                        if away {
                            palette.accent
                        } else {
                            palette.secondary
                        },
                    )
                },
            );
            if out_of_office.clicked() {
                actions.push(Action::OpenOutOfOffice);
                ui.close();
            }
            let paused_until = app.settings.notify.paused_until;
            let now = crate::time::now();
            if crate::notify::schedule::is_paused(paused_until, now) {
                let until = paused_until.map(crate::time::clock).unwrap_or_default();
                menu_row(
                    ui,
                    &palette,
                    &format!("Notifications paused until {until}"),
                    palette.secondary,
                    |ui, at| paint_icon(ui, Icon::BellOff, at, palette.accent),
                );
                let resume = menu_row(
                    ui,
                    &palette,
                    "Resume notifications",
                    palette.text,
                    |ui, at| paint_icon(ui, Icon::Bell, at, palette.secondary),
                );
                if resume.clicked() {
                    actions.push(Action::PauseNotifications(None));
                    ui.close();
                }
            } else {
                for (label, choice, icon) in [
                    (
                        "Pause notifications for 1 hour",
                        PauseFor::OneHour,
                        Icon::BellOff,
                    ),
                    (
                        "Pause notifications for 2 hours",
                        PauseFor::TwoHours,
                        Icon::BellOff,
                    ),
                    (
                        "Pause notifications until tomorrow",
                        PauseFor::UntilTomorrow,
                        Icon::BellOff,
                    ),
                ] {
                    let row = menu_row(ui, &palette, label, palette.secondary, |ui, at| {
                        paint_icon(ui, icon, at, palette.secondary)
                    });
                    if row.clicked() {
                        actions.push(Action::PauseNotifications(Some(choice)));
                        ui.close();
                    }
                }
            }
        });
    app.actions.extend(actions);
}

/// One clickable line of the account menu: highlighted under the pointer,
/// with a mark drawn by `leading` at the left and the label after it.
fn menu_row(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    label: &str,
    color: egui::Color32,
    leading: impl FnOnce(&egui::Ui, egui::Pos2),
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(268.0, 28.0), Sense::click());
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, CornerRadius::same(6), palette.surface_hover);
    }
    leading(ui, egui::pos2(rect.left() + 16.0, rect.center().y));
    ui.painter().text(
        egui::pos2(rect.left() + 32.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        theme::regular(13.0),
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

fn paint_icon(ui: &egui::Ui, icon: Icon, at: egui::Pos2, color: egui::Color32) {
    icon.image(color, 12.0).paint_at(
        ui,
        egui::Rect::from_center_size(at, egui::Vec2::splat(12.0)),
    );
}
