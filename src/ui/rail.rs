//! The navigation rail: Chats (under the app's logo), Teams, Calendar,
//! Calls, and the account.

/// The app icon in colour, for the rail's first item.
pub const APP_ICON: &str = "bytes://chaat-icon.svg";

use egui::{CornerRadius, Frame, Sense, vec2};

use crate::model::Availability;

use super::widgets;
use crate::app::App;
use crate::model::{Action, Section};
use crate::theme::{self, Icon};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    egui::Panel::left("rail")
        .exact_size(theme::RAIL_WIDTH)
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new().fill(palette.rail))
        .show(ui, |ui| {
            let top = egui::Rect::from_min_size(
                ui.max_rect().min,
                vec2(theme::RAIL_WIDTH, theme::HEADER_HEIGHT),
            );
            widgets::window_drag(ui, top, "rail-drag");
            // macOS keeps the header's height free for the traffic lights;
            // elsewhere the first item sits at the top.
            ui.add_space(if cfg!(target_os = "macos") {
                theme::HEADER_HEIGHT + 4.0
            } else {
                8.0
            });
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                for section in Section::ALL {
                    let badge = match section {
                        Section::Chats => app.unread_chats(),
                        _ => 0,
                    };
                    let response = item(ui, app, section, badge);
                    widgets::on_click(&response, &mut app.actions, Action::Navigate(section));
                }
            });
            // The account and settings at the bottom.
            ui.with_layout(egui::Layout::bottom_up(egui::Align::Center), |ui| {
                ui.add_space(12.0);
                let name = app
                    .profile
                    .as_ref()
                    .map(|p| p.display_name.clone())
                    .unwrap_or_else(|| "You".into());
                let me = app.me().to_owned();
                let avatar = widgets::avatar(
                    ui,
                    app,
                    Some(&me).filter(|m| !m.is_empty()).map(String::as_str),
                    &name,
                    32.0,
                    true,
                )
                .on_hover_text(format!("{name}\nAccounts and status"))
                .on_hover_cursor(egui::CursorIcon::PointingHand);
                // Another account has unread chats.
                if app.other_unread() > 0 {
                    ui.painter().circle_filled(
                        avatar.rect.right_top() + vec2(-3.0, 3.0),
                        4.5,
                        palette.accent,
                    );
                }
                if std::mem::take(&mut app.open_account_menu) {
                    egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&avatar));
                }
                account_menu(app, &avatar);
                ui.add_space(6.0);
                let settings = widgets::icon_button_sized(
                    ui,
                    &palette,
                    Icon::Settings,
                    "Settings (⌘,)",
                    36.0,
                    20.0,
                    palette.secondary,
                );
                widgets::on_click(&settings, &mut app.actions, Action::OpenSettings);
                if app.demo {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new("DEMO")
                            .font(theme::bold(11.0))
                            .color(palette.accent),
                    )
                    .on_hover_text("Sample data. Nothing is sent anywhere.");
                }
            });
        });
}

fn item(ui: &mut egui::Ui, app: &App, section: Section, badge: usize) -> egui::Response {
    let palette = &app.palette;
    let (rect, response) = ui.allocate_exact_size(vec2(56.0, 52.0), Sense::click());
    let active = app.section == section;
    if ui.is_rect_visible(rect) {
        if active || response.hovered() {
            ui.painter().rect_filled(
                rect.shrink2(vec2(2.0, 0.0)),
                CornerRadius::same(10),
                if active {
                    palette.surface_active
                } else {
                    palette.surface_hover
                },
            );
        }
        if active {
            let bar = egui::Rect::from_min_size(
                egui::pos2(rect.left() - 4.0, rect.top() + 14.0),
                vec2(3.0, 24.0),
            );
            ui.painter()
                .rect_filled(bar, CornerRadius::same(2), palette.accent);
        }
        let tint = if active {
            palette.accent
        } else {
            palette.secondary
        };
        let icon_rect = egui::Rect::from_center_size(
            rect.center_top() + vec2(0.0, 19.0),
            egui::Vec2::splat(21.0),
        );
        // Chats sits first and wears the app's own logo and name.
        let label = match section {
            Section::Chats => {
                egui::Image::new(APP_ICON)
                    .fit_to_exact_size(egui::Vec2::splat(26.0))
                    .paint_at(ui, icon_rect.expand(2.5));
                "Chaat"
            }
            other => {
                let icon = match other {
                    Section::Teams => Icon::Teams,
                    Section::Calendar => Icon::Calendar,
                    _ => Icon::Phone,
                };
                icon.image(tint, 21.0).paint_at(ui, icon_rect);
                other.label()
            }
        };
        ui.painter().text(
            rect.center_bottom() - vec2(0.0, 9.0),
            egui::Align2::CENTER_CENTER,
            label,
            theme::medium(11.0),
            if active {
                palette.text
            } else {
                palette.secondary
            },
        );
        if badge > 0 {
            widgets::badge(ui, palette, icon_rect.right_top() + vec2(3.0, 1.0), badge);
        }
    }
    let shortcut = match section {
        Section::Chats => "⌘1",
        Section::Teams => "⌘2",
        Section::Calendar => "⌘3",
        Section::Calls => "⌘4",
    };
    response
        .on_hover_text(format!("{} ({shortcut})", section.label()))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The avatar's menu: accounts, availability and status message. Settings
/// has its own button in the rail.
fn account_menu(app: &mut App, avatar: &egui::Response) {
    let palette = app.palette;
    let mut actions = Vec::new();
    egui::Popup::menu(avatar)
        .align(egui::RectAlign::RIGHT_END)
        .gap(8.0)
        .width(280.0)
        .show(|ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            // Accounts.
            for (index, account) in app.accounts.iter().enumerate() {
                let active = index == app.active;
                let (rect, response) = ui.allocate_exact_size(vec2(268.0, 46.0), Sense::click());
                if response.hovered() {
                    ui.painter()
                        .rect_filled(rect, CornerRadius::same(8), palette.surface_hover);
                }
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect.shrink2(vec2(8.0, 0.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                let label = account.label();
                let me = account.me().to_owned();
                // Avatars draw from the shown account's photos; others use
                // initials.
                widgets::avatar(
                    &mut child,
                    app,
                    if active {
                        Some(me.as_str()).filter(|m| !m.is_empty())
                    } else {
                        None
                    },
                    &label,
                    30.0,
                    active,
                );
                child.add_space(8.0);
                child.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(
                        egui::RichText::new(&label)
                            .font(theme::semibold(13.0))
                            .color(palette.text),
                    );
                    let detail = match &account.auth {
                        crate::backend::AuthStatus::SignedIn { username, .. } => username.clone(),
                        crate::backend::AuthStatus::Expired { .. } => "Sign in again".into(),
                        _ => "Signing in…".into(),
                    };
                    ui.label(
                        egui::RichText::new(detail)
                            .font(theme::regular(11.0))
                            .color(palette.dim),
                    );
                });
                child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if active {
                        theme::icon(ui, Icon::Check, 16.0, palette.accent);
                    } else if account.unread_chats() > 0 {
                        ui.label(
                            egui::RichText::new(account.unread_chats().to_string())
                                .font(theme::bold(11.0))
                                .color(palette.accent),
                        );
                    }
                });
                let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                if response.clicked() {
                    if !active {
                        actions.push(Action::SwitchAccount(index));
                    } else if matches!(account.auth, crate::backend::AuthStatus::Expired { .. }) {
                        actions.push(Action::SignIn);
                    }
                    ui.close();
                }
            }
            let add = menu_row(ui, &palette, "Add account", palette.text, |ui, at| {
                paint_icon(ui, Icon::Plus, at, palette.text);
            });
            if add.clicked() {
                actions.push(Action::AddAccount);
                ui.close();
            }
            ui.separator();
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
