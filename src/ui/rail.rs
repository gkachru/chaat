//! The navigation rail: Chats (under the app's logo), Teams, Calendar,
//! Calls, and the account.

/// The app icon in colour, for the rail's first item.
pub const APP_ICON: &str = "bytes://chaat-icon.svg";

pub(crate) mod editor;
mod menu;

use egui::{CornerRadius, Frame, Sense, vec2};

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
                // A clear gap between items, so a hovered item never looks
                // joined to the highlighted one.
                ui.spacing_mut().item_spacing.y = 12.0;
                accounts(app, ui);
                ui.separator();
                for section in Section::ALL {
                    let badge = match section {
                        Section::Chats if app.unified_view() => app.total_unread(),
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
                if std::mem::take(&mut app.open_account_menu) {
                    egui::Popup::open_id(ui.ctx(), egui::Popup::default_response_id(&avatar));
                }
                menu::account_menu(app, &avatar);
                ui.add_space(6.0);
                let open = app.settings_page.is_some();
                let settings = settings_button(ui, &palette, open);
                widgets::on_click(
                    &settings,
                    &mut app.actions,
                    if open {
                        Action::CloseSettings
                    } else {
                        Action::OpenSettings
                    },
                );
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

/// The logo (all accounts), one tile per account and the add button. Tiles
/// scroll when the window is too short; right-click opens the look editor.
fn accounts(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    // Room kept for the sections and the bottom buttons.
    let reserved = 140.0 + Section::ALL.len() as f32 * 56.0 + 56.0 + 24.0;
    let max_height = (ui.available_height() - reserved).max(48.0);
    let single = app.accounts.len() <= 1 || app.settings.unified_only;
    let logo_active = (single || app.unified) && app.settings_page.is_none();
    let (rect, logo) = ui.allocate_exact_size(vec2(48.0, 44.0), Sense::click());
    if ui.is_rect_visible(rect) {
        if logo_active {
            glass_tile(ui.painter(), &palette, rect, true);
        } else if logo.hovered() {
            glass_tile(ui.painter(), &palette, rect, false);
        }
        egui::Image::new(APP_ICON)
            .fit_to_exact_size(egui::Vec2::splat(26.0))
            .paint_at(
                ui,
                egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(26.0)),
            );
    }
    let logo = logo
        .on_hover_text("All accounts")
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    widgets::on_click(&logo, &mut app.actions, Action::ShowUnified);
    let mut actions = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt("rail-accounts")
        .max_height(max_height)
        .auto_shrink([true, true])
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            // The selection ring reaches 3 px outside a tile; keep it inside the clip.
            ui.add_space(4.0);
            let tiles = if single { 0 } else { app.accounts.len() };
            for index in 0..tiles {
                let look = app.account_look(index);
                let account = &app.accounts[index];
                let saved = account
                    .id
                    .as_ref()
                    .and_then(|id| app.settings.account_looks.get(id))
                    .cloned()
                    .unwrap_or_default();
                let selected = !app.unified && index == app.active;
                let expired = matches!(account.auth, crate::backend::AuthStatus::Expired { .. });
                let tile = widgets::account_tile(
                    ui,
                    &palette,
                    &look,
                    40.0,
                    selected,
                    account.unread_chats(),
                )
                .on_hover_text(if expired {
                    "Sign in again".to_owned()
                } else {
                    format!(
                        "{}
{}",
                        look.name,
                        account.username()
                    )
                })
                .on_hover_cursor(egui::CursorIcon::PointingHand);
                if expired {
                    ui.painter().circle(
                        tile.rect.right_top() + vec2(-3.0, 3.0),
                        5.0,
                        palette.warning,
                        egui::Stroke::new(1.5, palette.panel),
                    );
                }
                if tile.clicked() {
                    actions.push(Action::ShowAccount(index));
                }
                egui::Popup::context_menu(&tile).show(|ui| {
                    if let Some(new) = editor::look_editor(ui, &palette, &look, &saved) {
                        actions.push(Action::SetAccountLook(index, new));
                    }
                });
            }
            // Unified only: accounts are added from Settings › Account.
            if !app.settings.unified_only {
                let add = add_tile(ui, &palette)
                    .on_hover_text("Add account")
                    .on_hover_cursor(egui::CursorIcon::PointingHand);
                if add.clicked() {
                    actions.push(Action::AddAccount);
                }
            }
            ui.add_space(4.0);
        });
    app.actions.extend(actions);
}

/// The `+` tile: a dashed-looking outline with a plus.
fn add_tile(ui: &mut egui::Ui, palette: &theme::Palette) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let fill = if response.hovered() {
            palette.surface_hover
        } else {
            egui::Color32::TRANSPARENT
        };
        ui.painter().rect(
            rect,
            CornerRadius::same(10),
            fill,
            egui::Stroke::new(1.0, palette.dim),
            egui::StrokeKind::Inside,
        );
        Icon::Plus.image(palette.secondary, 16.0).paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(16.0)),
        );
    }
    response
}

fn item(ui: &mut egui::Ui, app: &App, section: Section, badge: usize) -> egui::Response {
    let palette = &app.palette;
    // Icon only: the name shows in the tooltip.
    let (rect, response) = ui.allocate_exact_size(vec2(48.0, 44.0), Sense::click());
    let active = app.section == section && app.settings_page.is_none();
    if ui.is_rect_visible(rect) {
        if active {
            glass_tile(ui.painter(), palette, rect, true);
        } else if response.hovered() {
            glass_tile(ui.painter(), palette, rect, false);
        }
        let tint = if active {
            palette.accent
        } else {
            palette.secondary
        };
        let icon_rect = egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(21.0));
        let icon = match section {
            Section::Chats => Icon::Chat,
            Section::Teams => Icon::Teams,
            Section::Calendar => Icon::Calendar,
            _ => Icon::Phone,
        };
        icon.image(tint, 21.0).paint_at(ui, icon_rect);
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

/// The Settings button, with the same tiles as the section items: raised
/// while settings are open.
fn settings_button(ui: &mut egui::Ui, palette: &theme::Palette, open: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(vec2(48.0, 44.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() || response.has_focus();
        if open {
            glass_tile(ui.painter(), palette, rect, true);
        } else if hovered {
            glass_tile(ui.painter(), palette, rect, false);
        }
        let tint = if open {
            palette.accent
        } else if hovered {
            palette.text
        } else {
            palette.secondary
        };
        Icon::Settings.image(tint, 20.0).paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), egui::Vec2::splat(20.0)),
        );
    }
    response
        .on_hover_text(if open {
            "Close settings (Esc)"
        } else {
            "Settings (⌘,)"
        })
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A rail item's glassy tile: a fill that is lighter at the top, a thin
/// light edge, and for the selected item a faint shadow. Colors derive from
/// the palette, so shared palettes get the same look; hover is quieter.
fn glass_tile(painter: &egui::Painter, palette: &theme::Palette, rect: egui::Rect, active: bool) {
    const RADIUS: u8 = 12;
    let base = if active {
        palette.surface_active
    } else {
        palette.surface_hover
    };
    // Light toward the top: the text color on dark themes, white on light.
    let (top, edge) = if palette.dark {
        (
            base.lerp_to_gamma(palette.text, if active { 0.10 } else { 0.06 }),
            palette
                .text
                .gamma_multiply(if active { 0.16 } else { 0.09 }),
        )
    } else {
        (
            base.lerp_to_gamma(egui::Color32::WHITE, if active { 0.65 } else { 0.5 }),
            egui::Color32::BLACK.gamma_multiply(if active { 0.10 } else { 0.06 }),
        )
    };
    if active {
        painter.add(
            egui::epaint::Shadow {
                offset: [0, 2],
                blur: 8,
                spread: 0,
                color: palette.shadow.gamma_multiply(0.6),
            }
            .as_shape(rect, CornerRadius::same(RADIUS)),
        );
    }
    // The gradient: a fan over the rounded outline, each point colored by
    // its height. The edge stroke on top hides the unfeathered outline.
    let mut outline = Vec::new();
    egui::epaint::tessellator::path::rounded_rectangle(
        &mut outline,
        rect,
        CornerRadius::same(RADIUS).into(),
    );
    let shade =
        |y: f32| top.lerp_to_gamma(base, ((y - rect.top()) / rect.height()).clamp(0.0, 1.0));
    let mut mesh = egui::epaint::Mesh::default();
    mesh.colored_vertex(rect.center(), shade(rect.center().y));
    for point in &outline {
        mesh.colored_vertex(*point, shade(point.y));
    }
    let count = outline.len() as u32;
    for i in 0..count {
        mesh.add_triangle(0, 1 + i, 1 + (i + 1) % count);
    }
    painter.add(mesh);
    painter.rect_stroke(
        rect,
        CornerRadius::same(RADIUS),
        egui::Stroke::new(1.0, edge),
        egui::StrokeKind::Inside,
    );
}
