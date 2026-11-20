//! Settings: a list of pages in place of the sidebar, and the chosen page in
//! place of the open conversation.

use egui::{CornerRadius, Frame, Margin, Sense, Stroke, vec2};

use super::widgets::{self, PointerControls as _};
use crate::app::App;
use crate::backend::AuthStatus;
use crate::calling::CallsOpenIn;
use crate::calling::browser::BrowserKind;
use crate::model::{
    Access, Action, AutoReplies, ExternalAudience, Millis, OutOfOfficeForm, SettingsPage,
};
use crate::settings::ThemeChoice;
use crate::theme::{self, Icon, Palette};

/// Below this width (beside the rail) the page list shows icons only.
const COMPACT_BELOW: f32 = 600.0;
const NAV_WIDTH: f32 = 208.0;
const NAV_COMPACT_WIDTH: f32 = 60.0;
/// Pages read best at a limited line length.
const PAGE_MAX_WIDTH: f32 = 640.0;

/// Pages this session offers: push updates need a real account.
fn pages(app: &App) -> impl Iterator<Item = SettingsPage> + '_ {
    SettingsPage::ALL
        .into_iter()
        .filter(|page| !(app.demo && *page == SettingsPage::Push))
}

fn icon(page: SettingsPage) -> Icon {
    match page {
        SettingsPage::Account => Icon::User,
        SettingsPage::OutOfOffice => Icon::Calendar,
        SettingsPage::General => Icon::Settings,
        SettingsPage::Appearance => Icon::Sun,
        SettingsPage::Calls => Icon::Phone,
        SettingsPage::Push => Icon::Bell,
        SettingsPage::Shortcuts => Icon::Keyboard,
        SettingsPage::Plugins => Icon::Settings,
        SettingsPage::Advanced => Icon::Lock,
        SettingsPage::About => Icon::Info,
    }
}

/// The list of pages, where the sidebar usually is.
pub fn nav(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let compact = ui.available_width() < COMPACT_BELOW;
    let current = app.settings_page.unwrap_or_default();
    let response = egui::Panel::left("settings-nav")
        .exact_size(if compact {
            NAV_COMPACT_WIDTH
        } else {
            NAV_WIDTH
        })
        .resizable(false)
        .show_separator_line(false)
        .frame(Frame::new().fill(palette.panel))
        .show(ui, |ui| {
            let header = egui::Rect::from_min_size(
                ui.max_rect().min,
                vec2(ui.max_rect().width(), theme::HEADER_HEIGHT),
            );
            widgets::window_drag(ui, header, "settings-nav-drag");
            if !compact {
                let inset = super::header_inset(ui.ctx(), theme::RAIL_WIDTH);
                ui.scope_builder(
                    egui::UiBuilder::new().max_rect(header.shrink2(vec2(12.0, 0.0))),
                    |ui| {
                        ui.horizontal_centered(|ui| {
                            ui.add_space(inset + 4.0);
                            ui.label(
                                egui::RichText::new("Settings")
                                    .font(theme::bold(20.0))
                                    .color(palette.text),
                            );
                        });
                    },
                );
            } else {
                ui.add_space(theme::HEADER_HEIGHT);
            }
            ui.add_space(4.0);
            let mut chosen = None;
            ui.vertical_centered(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                for page in pages(app).collect::<Vec<_>>() {
                    if nav_item(ui, &palette, page, page == current, compact).clicked() {
                        chosen = Some(page);
                    }
                }
            });
            if let Some(page) = chosen {
                app.actions.push(Action::ShowSettingsPage(page));
            }
            if !compact {
                ui.with_layout(egui::Layout::bottom_up(egui::Align::Min), |ui| {
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        ui.add_space(16.0);
                        ui.label(
                            egui::RichText::new(format!("Chaat {}", env!("CARGO_PKG_VERSION")))
                                .font(theme::regular(12.0))
                                .color(palette.dim),
                        );
                    });
                });
            }
        });
    let rect = response.response.rect;
    ui.painter().vline(
        rect.right(),
        rect.y_range(),
        Stroke::new(1.0, palette.outline),
    );
}

/// One page in the list: icon and name, or the icon alone when compact.
fn nav_item(
    ui: &mut egui::Ui,
    palette: &Palette,
    page: SettingsPage,
    selected: bool,
    compact: bool,
) -> egui::Response {
    let size = if compact {
        vec2(44.0, 36.0)
    } else {
        vec2(ui.available_width() - 16.0, 34.0)
    };
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let painter = ui.painter();
        let hovered = response.hovered() || response.has_focus();
        if selected {
            painter.rect_filled(rect, CornerRadius::same(8), palette.surface_active);
            let bar = egui::Rect::from_min_size(
                rect.left_top() + vec2(0.0, 8.0),
                vec2(3.0, rect.height() - 16.0),
            );
            painter.rect_filled(bar, CornerRadius::same(2), palette.accent);
        } else if hovered {
            painter.rect_filled(rect, CornerRadius::same(8), palette.surface_hover);
        }
        let tint = if selected {
            palette.accent
        } else {
            palette.secondary
        };
        let icon_center = if compact {
            rect.center()
        } else {
            egui::pos2(rect.left() + 22.0, rect.center().y)
        };
        icon(page).image(tint, 16.0).paint_at(
            ui,
            egui::Rect::from_center_size(icon_center, egui::Vec2::splat(16.0)),
        );
        if !compact {
            ui.painter().text(
                egui::pos2(rect.left() + 40.0, rect.center().y),
                egui::Align2::LEFT_CENTER,
                page.label(),
                if selected {
                    theme::semibold(14.0)
                } else {
                    theme::regular(14.0)
                },
                if selected {
                    palette.text
                } else {
                    palette.secondary
                },
            );
        }
    }
    let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
    if compact {
        response.on_hover_text(page.label())
    } else {
        response
    }
}

/// The chosen page, where the open conversation usually is.
pub fn page(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    open_plugins_for_setup(app, ui.ctx());
    let page = app.settings_page.unwrap_or_default();
    let mut actions = Vec::new();
    if page != SettingsPage::Plugins {
        // An unsaved plugin secret does not outlive its page.
        super::plugins::forget_secret_buffers(ui.ctx());
    }
    egui::CentralPanel::no_frame()
        .frame(Frame::new().fill(palette.window))
        .show(ui, |ui| {
            let header = egui::Rect::from_min_size(
                ui.max_rect().min,
                vec2(ui.max_rect().width(), theme::HEADER_HEIGHT),
            );
            widgets::window_drag(ui, header, "settings-page-drag");
            let left = super::header_inset(ui.ctx(), header.left());
            let right = super::header_right_inset(ui.ctx());
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(egui::Rect::from_min_max(
                    header.min + vec2(28.0 + left, 0.0),
                    header.max - vec2(16.0 + right, 0.0),
                )),
                |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.label(
                            egui::RichText::new(page.label())
                                .font(theme::bold(20.0))
                                .color(palette.text),
                        );
                    });
                },
            );
            egui::ScrollArea::vertical()
                .id_salt(("settings-page", page))
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    Frame::new()
                        .inner_margin(Margin {
                            left: 28,
                            right: 28,
                            top: 0,
                            bottom: 28,
                        })
                        .show(ui, |ui| {
                            ui.set_max_width(ui.available_width().min(PAGE_MAX_WIDTH));
                            // One size up from the app's defaults, for labels,
                            // checkboxes, buttons and menus.
                            let styles = &mut ui.style_mut().text_styles;
                            styles.insert(egui::TextStyle::Body, theme::regular(15.0));
                            styles.insert(egui::TextStyle::Button, theme::regular(14.0));
                            ui.spacing_mut().item_spacing.y = 8.0;
                            match page {
                                SettingsPage::Account => account(app, ui, &mut actions),
                                SettingsPage::OutOfOffice => out_of_office(app, ui, &mut actions),
                                SettingsPage::General => general(app, ui, &mut actions),
                                SettingsPage::Appearance => appearance(app, ui, &mut actions),
                                SettingsPage::Calls => calls(app, ui, &mut actions),
                                SettingsPage::Push => push(app, ui, &mut actions),
                                SettingsPage::Shortcuts => shortcuts(app, ui),
                                SettingsPage::Plugins => {
                                    super::plugins::page(app, ui, &mut actions)
                                }
                                SettingsPage::Advanced => advanced(app, ui, &mut actions),
                                SettingsPage::About => about(app, ui, &mut actions),
                            }
                        });
                });
        });
    app.actions.extend(actions);
}

/// A plugin's setup view lives on the Plugins page: when a setup opens (or
/// was open before Settings was drawn), the page changes to Plugins once.
fn open_plugins_for_setup(app: &mut App, ctx: &egui::Context) {
    let frame = ctx.cumulative_frame_nr();
    let key = egui::Id::new("settings-setup-seen");
    // What the last frame saw; nothing if Settings was not drawn then.
    let seen = ctx
        .data(|d| d.get_temp::<(u64, Option<String>)>(key))
        .filter(|(last, _)| frame <= last.saturating_add(1))
        .and_then(|(_, setup)| setup);
    if app.plugin_setup.is_some() && app.plugin_setup != seen {
        app.settings_page = Some(SettingsPage::Plugins);
    }
    ctx.data_mut(|d| d.insert_temp(key, (frame, app.plugin_setup.clone())));
}

/// A group's heading inside a page: like [`widgets::section_label`], one
/// size up to match the page's text.
pub(super) fn group(ui: &mut egui::Ui, palette: &Palette, label: &str) {
    ui.add_space(14.0);
    ui.label(
        egui::RichText::new(label.to_uppercase())
            .font(theme::semibold(12.0))
            .color(palette.dim)
            .extra_letter_spacing(0.6),
    );
}

/// A quiet explanation under a setting.
pub(super) fn note(ui: &mut egui::Ui, palette: &Palette, text: impl Into<String>) {
    ui.add(
        egui::Label::new(
            egui::RichText::new(text.into())
                .font(theme::regular(13.0))
                .color(palette.dim),
        )
        .wrap(),
    );
}

fn account(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    group(ui, &palette, "Signed in");
    match &app.auth {
        AuthStatus::SignedIn { name, username, .. } => {
            ui.horizontal(|ui| {
                let me = app.me().to_owned();
                widgets::avatar(ui, app, Some(&me), name, 40.0, true);
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.label(
                        egui::RichText::new(name)
                            .font(theme::semibold(15.0))
                            .color(palette.text),
                    );
                    ui.label(egui::RichText::new(username).color(palette.secondary));
                });
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if !app.demo
                        && widgets::secondary_button(ui, &palette, "Sign out")
                            .on_hover_text("Signs out and deletes this account's local cache")
                            .clicked()
                    {
                        actions.push(Action::SignOut);
                    }
                });
            });
        }
        other => {
            ui.label(
                egui::RichText::new(format!("Not signed in ({})", other.log_label()))
                    .color(palette.secondary),
            );
        }
    }
    ui.horizontal_wrapped(|ui| {
        let accounts = app.accounts.len();
        ui.label(
            egui::RichText::new(if accounts == 1 {
                "1 account in Chaat".to_owned()
            } else {
                format!("{accounts} accounts in Chaat · switch from your picture or with ⌘⇧A")
            })
            .font(theme::regular(13.0))
            .color(palette.dim),
        );
        if !app.demo && ui.link("Add account").clicked() {
            actions.push(Action::AddAccount);
        }
    });
    if !app.demo {
        group(ui, &palette, "Permissions");
        let channel = match app.channel_access {
            Access::Granted => "granted",
            Access::NeedsConsent => "needs admin consent",
            Access::Forbidden => "refused by the tenant",
            Access::Unknown => "unknown",
        };
        note(ui, &palette, format!("Channel posts: {channel}"));
        note(
            ui,
            &palette,
            format!(
                "Granted permissions: {}",
                if app.granted.is_empty() {
                    "–"
                } else {
                    &app.granted
                }
            ),
        );
    }
}

/// Automatic replies (Out of Office), kept in the Exchange mailbox.
fn out_of_office(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    note(
        ui,
        &palette,
        "Automatic replies answer people while you're away. Teams shows you as out of office while they're on, and Outlook sends the same replies by email.",
    );
    if !app.has_scope(crate::auth::MAILBOX_SETTINGS_SCOPE) {
        group(ui, &palette, "Permission");
        note(
            ui,
            &palette,
            "Chaat needs the MailboxSettings.ReadWrite permission to read and set your automatic replies.",
        );
        if widgets::primary_button(ui, &palette, "  Allow access  ").clicked() {
            actions.push(Action::OpenOutOfOffice);
        }
        return;
    }
    if app.no_mailbox {
        ui.add_space(8.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "This account has no Exchange Online mailbox, so it has no automatic replies to set.",
                )
                .color(palette.warning),
            )
            .wrap(),
        );
        return;
    }
    let Some(saved) = app.out_of_office.clone() else {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            widgets::loading(ui, palette.dim, 16.0);
            ui.label(
                egui::RichText::new("Loading your automatic replies…").color(palette.secondary),
            );
        });
        return;
    };
    let saving = app.out_of_office_saving;
    let now = crate::time::now();
    let form = &mut app.form_out_of_office;

    group(ui, &palette, "Automatic replies");
    ui.add_enabled_ui(!saving, |ui| {
        ui.horizontal_wrapped(|ui| {
            for (replies, label) in [
                (AutoReplies::Off, "Off"),
                (AutoReplies::On, "On"),
                (AutoReplies::Scheduled, "On during a time range"),
            ] {
                ui.radio_hand(&mut form.replies, replies, label);
                ui.add_space(8.0);
            }
        });
        if form.replies == AutoReplies::Scheduled {
            egui::Grid::new("out-of-office-window")
                .num_columns(3)
                .spacing(vec2(10.0, 8.0))
                .show(ui, |ui| {
                    for (label, date, time) in [
                        ("Starts", &mut form.start_date, &mut form.start_time),
                        ("Ends", &mut form.end_date, &mut form.end_time),
                    ] {
                        ui.label(egui::RichText::new(label).color(palette.secondary));
                        ui.add(
                            widgets::text_field(date, "YYYY-MM-DD", theme::regular(14.0))
                                .desired_width(120.0),
                        );
                        egui::ComboBox::from_id_salt(("out-of-office-time", label))
                            .selected_text(time.as_str())
                            .width(80.0)
                            .show_ui(ui, |ui| {
                                for slot in
                                    (0..48).map(|n| format!("{:02}:{:02}", n / 2, n % 2 * 30))
                                {
                                    let on = *time == slot;
                                    if ui.selectable_label(on, &slot).clicked() {
                                        *time = slot;
                                    }
                                }
                            })
                            .response
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        ui.end_row();
                    }
                });
            note(ui, &palette, "Times are in this computer's time zone.");
        }

        group(ui, &palette, "Inside your organization");
        ui.add(
            egui::TextEdit::multiline(&mut form.internal)
                .hint_text("Away until Monday. I'll reply when I'm back.")
                .desired_rows(3)
                .desired_width(f32::INFINITY)
                .margin(Margin::symmetric(10, 8)),
        );

        group(ui, &palette, "Outside your organization");
        egui::ComboBox::from_id_salt("out-of-office-audience")
            .selected_text(form.audience.label())
            .width(300.0)
            .show_ui(ui, |ui| {
                for audience in ExternalAudience::ALL {
                    ui.selectable_value(&mut form.audience, audience, audience.label());
                }
            })
            .response
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        if form.audience != ExternalAudience::None {
            ui.add(
                egui::TextEdit::multiline(&mut form.external)
                    .hint_text("Away until Monday. I'll reply when I'm back.")
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(10, 8)),
            );
        }
    });

    ui.add_space(10.0);
    let value = form.value();
    let changed = form.changed_from(&saved);
    ui.horizontal(|ui| {
        let ready = changed && value.is_ok() && !saving;
        let save = ui
            .add_enabled_ui(ready, |ui| {
                widgets::primary_button(
                    ui,
                    &palette,
                    if saving { "  Saving…  " } else { "  Save  " },
                )
            })
            .inner;
        if let (true, Ok(value)) = (save.clicked(), &value) {
            actions.push(Action::SetOutOfOffice(value.clone()));
        }
        if changed
            && !saving
            && widgets::secondary_button(ui, &palette, "Discard changes").clicked()
        {
            *form = OutOfOfficeForm::new(&saved, now);
        }
    });
    if let Err(reason) = value {
        ui.label(
            egui::RichText::new(reason)
                .font(theme::regular(13.0))
                .color(palette.warning),
        );
    }
    let state = match saved.replies {
        AutoReplies::Off => "Automatic replies are off.".to_owned(),
        AutoReplies::On => "Automatic replies are on.".to_owned(),
        AutoReplies::Scheduled => {
            let at = |millis: Option<Millis>| {
                millis
                    .map(|at| {
                        let (date, time) = crate::time::local_date_time(at);
                        format!("{date} {time}")
                    })
                    .unwrap_or_else(|| "?".into())
            };
            format!(
                "Automatic replies are scheduled from {} to {}{}.",
                at(saved.start),
                at(saved.end),
                if saved.active(now) { " (on now)" } else { "" }
            )
        }
    };
    note(ui, &palette, state);
}

fn general(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    group(ui, &palette, "Window");
    if cfg!(any(windows, target_os = "linux")) {
        let mut own = app.settings.custom_titlebar;
        if ui
            .checkbox_hand(&mut own, "Use Chaat's own title bar")
            .on_hover_text(
                "Draws minimize, maximize and close in Chaat's header. Off: the system's window frame.",
            )
            .changed()
        {
            actions.push(Action::SetCustomTitlebar(own));
        }
    }
    let mut keep = app.settings.keep_running;
    if ui
        .checkbox_hand(&mut keep, "Keep running in the tray when the window closes")
        .changed()
    {
        actions.push(Action::SetKeepRunning(keep));
    }

    group(ui, &palette, "Notifications");
    let mut notifications = app.settings.notifications;
    if ui
        .checkbox_hand(
            &mut notifications,
            "Desktop notifications for new messages and meeting reminders",
        )
        .changed()
    {
        actions.push(Action::SetNotifications(notifications));
    }

    group(ui, &palette, "Presence");
    let mut session = app.settings.presence_session;
    if ui
        .checkbox_hand(&mut session, "Show me as available while I use Chaat")
        .changed()
    {
        actions.push(Action::SetPresenceSession(session));
    }
    note(
        ui,
        &palette,
        "Keeps a Microsoft presence session, as a Teams app does: you show Available while you use Chaat, and Away a few minutes after you stop. Needs the Presence.ReadWrite permission, asked for the first time you set your status.",
    );

    group(ui, &palette, "Fetching");
    note(
        ui,
        &palette,
        "New messages are fetched when you open a chat, come back to the window or press ⌘R. Microsoft's rules for Teams forbid background polling; for instant alerts, connect a push relay (docs/realtime.md).",
    );
    if !app.demo && ui.link("Set up push updates").clicked() {
        actions.push(Action::ShowSettingsPage(SettingsPage::Push));
    }
}

fn appearance(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    group(ui, &palette, "Theme");
    ui.horizontal_wrapped(|ui| {
        for (choice, label, icon) in [
            (ThemeChoice::System, "System", Icon::Monitor),
            (ThemeChoice::Light, "Light", Icon::Sun),
            (ThemeChoice::Dark, "Dark", Icon::Moon),
        ] {
            let on = app.settings.theme == choice;
            let response = widgets::pill(ui, &palette, icon, label, on);
            widgets::on_click(&response, actions, Action::SetTheme(choice));
        }
    });

    group(ui, &palette, "Palette");
    let current = match &app.settings.theme {
        ThemeChoice::Palette(name) => fastframe_theme::display_name(name).to_owned(),
        _ => "Chaat".to_owned(),
    };
    egui::ComboBox::from_id_salt("settings-palette")
        .selected_text(current)
        .width(240.0)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(
                    !matches!(app.settings.theme, ThemeChoice::Palette(_)),
                    "Chaat",
                )
                .clicked()
            {
                actions.push(Action::SetTheme(ThemeChoice::System));
            }
            for preset in theme::presets() {
                let on = app.settings.theme == ThemeChoice::Palette(preset.filename.clone());
                if ui
                    .selectable_label(on, fastframe_theme::display_name(&preset.filename))
                    .clicked()
                {
                    actions.push(Action::SetTheme(ThemeChoice::Palette(
                        preset.filename.clone(),
                    )));
                }
            }
        })
        .response
        .on_hover_cursor(egui::CursorIcon::PointingHand);

    group(ui, &palette, "Zoom");
    ui.label(egui::RichText::new(format!("{:.0}%", app.settings.zoom * 100.0)).color(palette.text));
    note(
        ui,
        &palette,
        super::keys::platform("⌘+ zooms in, ⌘− zooms out, ⌘0 resets."),
    );
}

fn calls(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let browsers: Vec<BrowserKind> = app.call_browsers().iter().map(|b| b.kind).collect();
    let mut calls = app.settings.calls.clone();

    group(ui, &palette, "Opening calls");
    egui::ComboBox::from_label(egui::RichText::new("Open in").color(palette.secondary))
        .selected_text(calls.open_in.label())
        .width(300.0)
        .show_ui(ui, |ui| {
            for choice in CallsOpenIn::ALL {
                ui.selectable_value(&mut calls.open_in, choice, choice.label());
            }
        })
        .response
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    let app_window = calls.open_in == CallsOpenIn::AppWindow;
    if !browsers.is_empty() && (app_window || calls.links_in_browser) {
        let current = calls
            .browser
            .filter(|b| browsers.contains(b))
            .unwrap_or(browsers[0]);
        egui::ComboBox::from_label(egui::RichText::new("Browser").color(palette.secondary))
            .selected_text(current.name())
            .show_ui(ui, |ui| {
                for kind in &browsers {
                    if ui.selectable_label(current == *kind, kind.name()).clicked() {
                        calls.browser = Some(*kind);
                    }
                }
            })
            .response
            .on_hover_cursor(egui::CursorIcon::PointingHand);
    }
    if let [first, ..] = browsers.as_slice() {
        let current = calls
            .browser
            .filter(|b| browsers.contains(b))
            .unwrap_or(*first);
        ui.checkbox_hand(
            &mut calls.links_in_browser,
            format!("Open all links in {}", current.short()),
        )
        .on_hover_text(
            "Links in messages and the calendar open in this browser instead of your default one, in the same profile as calls.",
        );
    }
    if app_window {
        match browsers.as_slice() {
            [] => {
                ui.label(
                    egui::RichText::new("A separate window needs Microsoft Edge or Google Chrome. Until one is installed, calls open in your default browser.")
                        .font(theme::regular(13.0))
                        .color(palette.warning),
                );
            }
            found => {
                let current = calls
                    .browser
                    .filter(|b| found.contains(b))
                    .unwrap_or(found[0]);
                group(ui, &palette, "Call window");
                ui.checkbox_hand(
                    &mut calls.dock,
                    "Move Chaat beside the call window during calls",
                );
                if app.accounts.len() > 1 || calls.profile_per_account {
                    ui.checkbox_hand(
                        &mut calls.profile_per_account,
                        "A separate browser profile for each account",
                    )
                    .on_hover_text("Each account signs in to Microsoft in its own browser profile, so Teams on the web never asks which account to use.");
                }
                ui.checkbox_hand(
                    &mut calls.sign_out_browser,
                    format!(
                        "Signing out of Chaat also signs {} out of Microsoft",
                        current.short()
                    ),
                )
                .on_hover_text(
                    "Signs that browser out of every Microsoft site, including Teams on the web.",
                );

                group(ui, &palette, "Teams on the web");
                note(
                    ui,
                    &palette,
                    format!(
                        "Calls run in Teams on the web, Microsoft's own client, so they cost nothing extra. Chaat signs in through {} too, so Teams there finds your Microsoft session. To stay signed in, sign in to Teams on the web once and choose \"Stay signed in\".",
                        current.short()
                    ),
                );
                if widgets::secondary_button(ui, &palette, "Sign in to Teams on the web").clicked()
                {
                    actions.push(Action::PrepareTeamsWeb);
                }
            }
        }
    }
    call_alerts(app, ui, actions);
    if calls != app.settings.calls {
        actions.push(Action::SetCallSettings(calls));
    }
}

/// Call alerts come from a plugin: one line here, the rest in Plugins.
fn call_alerts(app: &App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    group(ui, &palette, "Call alerts");
    // Chaat's own words, whatever a plugin says about itself.
    note(
        ui,
        &palette,
        "Microsoft Graph can't tell Chaat that a call is ringing. A plugin can: a separate program you install, not part of Chaat. Add and manage plugins in Settings \u{203A} Plugins.",
    );
    if widgets::secondary_button(ui, &palette, "Open Plugins").clicked() {
        actions.push(Action::ShowSettingsPage(SettingsPage::Plugins));
    }
}

fn push(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    if app.demo {
        note(ui, &palette, "Push updates need a signed-in account.");
        return;
    }
    note(
        ui,
        &palette,
        "With a chaat-relay you run, Microsoft Graph tells Chaat about new messages as they happen, also in the background. Only chat and message IDs pass through the relay.",
    );
    if let Some(view) = app.push_plugin_view() {
        push_plugin(app, ui, &view, actions);
    }
    group(ui, &palette, "Relay");
    ui.horizontal(|ui| {
        ui.add(
            widgets::text_field(
                &mut app.form_relay,
                "https://relay.example.com",
                theme::regular(14.0),
            )
            .desired_width(ui.available_width() - 70.0),
        );
        let changed =
            app.form_relay.trim() != app.settings.relay_url.as_deref().unwrap_or_default();
        if ui.add_enabled(changed, egui::Button::new("Save")).clicked() {
            actions.push(Action::SetRelay(app.form_relay.clone()));
        }
    });
    if app.settings.relay_key_set {
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new("Access key saved in the system's credential store.")
                    .font(theme::regular(13.0))
                    .color(palette.secondary),
            );
            if ui.button("Remove").clicked() {
                actions.push(Action::SetRelayKey(None));
            }
        });
    } else {
        ui.horizontal(|ui| {
            ui.add(
                widgets::text_field(
                    &mut app.form_relay_key,
                    "Access key, if the relay needs one",
                    theme::regular(14.0),
                )
                .password(true)
                .desired_width(ui.available_width() - 70.0),
            );
            let typed = !app.form_relay_key.trim().is_empty();
            if ui.add_enabled(typed, egui::Button::new("Save")).clicked() {
                actions.push(Action::SetRelayKey(Some(app.form_relay_key.clone())));
            }
        });
    }
    group(ui, &palette, "Status");
    let color = match app.push {
        crate::realtime::PushStatus::Live => palette.success,
        crate::realtime::PushStatus::Failed(_) => palette.danger,
        _ => palette.secondary,
    };
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(app.push.label()).color(color));
        // Chaat retries on its own; this skips the wait.
        if matches!(app.push, crate::realtime::PushStatus::Failed(_))
            && ui
                .button("Retry now")
                .on_hover_text("Connect to the relay again")
                .clicked()
        {
            actions.push(Action::RetryRelay);
        }
    });
}

/// What the push plugin says about its tunnel. The plugin's name and reason
/// are quoted, so they read as the plugin's words and not Chaat's.
fn push_plugin(
    app: &App,
    ui: &mut egui::Ui,
    view: &crate::app::PushPluginView,
    actions: &mut Vec<Action>,
) {
    use crate::app::PushPluginState;
    let palette = app.palette;
    group(ui, &palette, "Plugin");
    ui.add(
        egui::Label::new(
            egui::RichText::new(format!("Plugin: \u{201C}{}\u{201D}", view.name))
                .font(theme::regular(14.0))
                .color(palette.text),
        )
        .wrap(),
    );
    if view.relay_set {
        note(
            ui,
            &palette,
            format!(
                "Plugin \u{201C}{}\u{201D} is not used: the relay address is used instead.",
                view.name
            ),
        );
        return;
    }
    let (text, color) = match &view.state {
        PushPluginState::Up { host } => (format!("Tunnel at {host}"), palette.success),
        PushPluginState::Waiting => (
            "Waiting for the plugin's tunnel".to_owned(),
            palette.secondary,
        ),
        PushPluginState::Down { .. } => ("The plugin's tunnel is down".to_owned(), palette.danger),
        PushPluginState::NeedsApproval => (
            "Allow it in Settings \u{203A} Plugins".to_owned(),
            palette.secondary,
        ),
    };
    ui.add(
        egui::Label::new(
            egui::RichText::new(text)
                .font(theme::regular(13.0))
                .color(color),
        )
        .wrap(),
    );
    if let PushPluginState::Down {
        reason: Some(reason),
    } = &view.state
    {
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("\u{201C}{reason}\u{201D}"))
                    .font(theme::regular(13.0))
                    .color(palette.secondary),
            )
            .wrap(),
        );
    }
    if view.state == PushPluginState::NeedsApproval
        && widgets::secondary_button(ui, &palette, "Open Plugins").clicked()
    {
        actions.push(Action::ShowSettingsPage(SettingsPage::Plugins));
    }
}

fn shortcuts(app: &App, ui: &mut egui::Ui) {
    let palette = app.palette;
    for (name, list) in super::keys::SHORTCUTS {
        group(ui, &palette, name);
        egui::Grid::new(("shortcuts", *name))
            .num_columns(2)
            .min_col_width(180.0)
            .spacing(vec2(24.0, 8.0))
            .show(ui, |ui| {
                for (keys, what) in *list {
                    ui.label(
                        egui::RichText::new(super::keys::platform(keys))
                            .font(theme::mono(13.0))
                            .color(palette.accent),
                    );
                    ui.add(egui::Label::new(egui::RichText::new(*what).color(palette.text)).wrap());
                    ui.end_row();
                }
            });
    }
}

fn advanced(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    if !app.demo {
        group(ui, &palette, "App registration");
        ui.label(egui::RichText::new("Application (client) ID").color(palette.secondary));
        ui.add(widgets::text_field(
            &mut app.form_client_id,
            "",
            theme::mono(14.0),
        ));
        ui.label(egui::RichText::new("Tenant").color(palette.secondary));
        ui.add(widgets::text_field(
            &mut app.form_tenant,
            "",
            theme::regular(14.0),
        ));
        let changed = app.form_client_id.trim() != app.settings.client_id
            || app.form_tenant.trim() != app.settings.tenant;
        if ui
            .add_enabled(changed, egui::Button::new("Save and restart sign-in"))
            .clicked()
        {
            actions.push(Action::SaveAppRegistration {
                client_id: app.form_client_id.clone(),
                tenant: app.form_tenant.clone(),
            });
        }
        if crate::settings::is_guid(&app.settings.client_id) {
            let consent = format!(
                "https://login.microsoftonline.com/{}/adminconsent?client_id={}",
                if app.settings.tenant == "organizations" {
                    "common"
                } else {
                    &app.settings.tenant
                },
                app.settings.client_id
            );
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Admin consent link for your tenant's administrator:")
                        .font(theme::regular(13.0))
                        .color(palette.dim),
                );
                if ui.link("Copy").clicked() {
                    actions.push(Action::CopyText(consent));
                }
            });
        }
    }

    group(ui, &palette, "Diagnostics");
    let mut perf = app.settings.perf_overlay;
    if ui
        .checkbox_hand(
            &mut perf,
            super::keys::platform("Show the performance overlay (⌘⇧P)"),
        )
        .changed()
    {
        actions.push(Action::TogglePerfOverlay);
    }
    if ui.link("Log file folder").clicked() {
        actions.push(Action::OpenUrl(format!(
            "file://{}",
            app.dirs.state.display()
        )));
    }
}

fn about(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    group(ui, &palette, "Chaat");
    ui.label(
        egui::RichText::new(format!(
            "Chaat {} · based on Wren · MIT licence · no telemetry.",
            env!("CARGO_PKG_VERSION")
        ))
        .color(palette.text),
    );
    note(
        ui,
        &palette,
        "Works with Microsoft Teams. Not affiliated with Microsoft.",
    );

    group(ui, &palette, "Updates");
    let mut automatic = app.settings.automatic_updates;
    if ui
        .checkbox_hand(
            &mut automatic,
            "Automatically check for and download app updates",
        )
        .changed()
    {
        actions.push(Action::SetAutomaticUpdates(automatic));
    }
    note(
        ui,
        &palette,
        "Checks GitHub once a day. Updates are verified before installation; you choose when to restart.",
    );
    match &app.update {
        crate::updates::Status::Idle => {
            ui.label("No update check yet.");
        }
        crate::updates::Status::Checking => {
            ui.label("Checking for updates…");
        }
        crate::updates::Status::Current => {
            ui.label("You have the latest public release.");
        }
        crate::updates::Status::Available { release, reason } => {
            ui.label(format!("Chaat {} is available.", release.version));
            if let Some(reason) = reason {
                ui.label(reason);
            } else if ui.button("Download update").clicked() {
                actions.push(Action::DownloadUpdate);
            }
        }
        crate::updates::Status::Downloading { received, total } => {
            let fraction = if *total == 0 {
                0.0
            } else {
                *received as f32 / *total as f32
            };
            ui.add(egui::ProgressBar::new(fraction).text("Downloading and verifying update…"));
        }
        crate::updates::Status::Ready(_) => {
            ui.label("Update verified and ready.");
            if ui.button("Restart to update").clicked() {
                actions.push(Action::InstallUpdate);
            }
        }
        crate::updates::Status::Installing => {
            ui.label("Preparing to restart…");
        }
        crate::updates::Status::Failed(message) => {
            ui.label(egui::RichText::new(*message).color(palette.danger));
        }
    }
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !app.update.busy() && !matches!(app.update, crate::updates::Status::Ready(_)),
                egui::Button::new("Check for updates"),
            )
            .clicked()
        {
            actions.push(Action::CheckUpdate);
        }
        if ui.link("Release notes and downloads").clicked() {
            actions.push(Action::OpenUrl(
                "https://github.com/gkachru/chaat/releases".into(),
            ));
        }
    });
}

/// Called every frame the settings window is closed: nothing typed into it
/// stays in memory.
pub fn closed(ctx: &egui::Context) {
    super::plugins::forget_secret_buffers(ctx);
}
