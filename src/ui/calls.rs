//! Calls: start a call with someone, join a meeting by link, and where calls
//! run, with a way to change it.

use egui::{CornerRadius, Frame, Margin, Sense, Stroke, vec2};

use super::widgets;
use crate::app::App;
use crate::model::{Action, ChatKind, SettingsPage};
use crate::theme::{self, Icon};

/// People from one-on-one chats, most recent first.
pub fn list(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let unified = app.unified_view();
    let active = app.active;
    let indexes: Vec<usize> = if unified {
        (0..app.accounts.len()).collect()
    } else {
        vec![active]
    };
    // (account, user id, name, email), most recent chat first.
    let mut people: Vec<(usize, String, String, String, i64)> = Vec::new();
    for &j in &indexes {
        let account = &app.accounts[j];
        let me = account.me();
        people.extend(
            account
                .chats
                .iter()
                .filter(|c| c.kind == ChatKind::OneOnOne)
                .filter_map(|c| Some((c, c.peer(me)?)))
                .filter_map(|(c, m)| {
                    Some((
                        j,
                        m.user_id.clone(),
                        m.display_name.clone(),
                        m.email.clone()?,
                        c.updated,
                    ))
                })
                .take(40),
        );
    }
    if unified {
        people.sort_by_key(|p| std::cmp::Reverse(p.4));
        people.truncate(40);
    }
    ui.horizontal(|ui| {
        ui.add_space(18.0);
        widgets::section_label(ui, &palette, "Recent people");
    });
    ui.add_space(4.0);
    if people.is_empty() {
        ui.horizontal(|ui| {
            ui.add_space(18.0);
            ui.label(egui::RichText::new("People you chat with appear here.").color(palette.dim));
        });
        return;
    }
    let mut actions = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt("call-people")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            for (j, user, name, email, _) in &people {
                let j = *j;
                let (_, inner) = widgets::list_row(ui, &palette, 48.0, false, Sense::hover());
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(inner.shrink2(vec2(8.0, 0.0)))
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                );
                widgets::avatar(&mut child, app, Some(user), name, 32.0, true);
                child.add_space(8.0);
                if unified {
                    let (rect, _) = child.allocate_exact_size(vec2(16.0, 16.0), Sense::hover());
                    widgets::paint_account_mark(&child, &app.account_look(j), rect);
                    child.add_space(6.0);
                }
                let label_width = (child.available_width() - 72.0).max(40.0);
                child.add(egui::Label::new(super::widgets::elided(
                    &child,
                    name,
                    theme::medium(13.0),
                    palette.text,
                    label_width,
                )));
                child.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let video = widgets::icon_button(ui, &palette, Icon::Video, "Video call");
                    let audio = widgets::icon_button(ui, &palette, Icon::Phone, "Audio call");
                    for (response, with_video) in [(video, true), (audio, false)] {
                        if response.clicked() {
                            if j != active {
                                actions.push(Action::SwitchAccount(j));
                            }
                            actions.push(Action::StartCall {
                                users: vec![email.clone()],
                                video: with_video,
                            });
                        }
                    }
                });
            }
        });
    app.actions.extend(actions);
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let header = egui::Rect::from_min_size(
        ui.max_rect().min,
        vec2(ui.max_rect().width(), theme::HEADER_HEIGHT),
    );
    widgets::window_drag(ui, header, "calls-drag");
    ui.add_space(theme::HEADER_HEIGHT + 12.0);
    egui::ScrollArea::vertical().id_salt("calls-page").auto_shrink([false, false]).show(ui, |ui| {
        Frame::new().inner_margin(Margin::symmetric(32, 0)).show(ui, |ui| {
            ui.set_max_width(ui.available_width().min(680.0));
            ui.label(egui::RichText::new("Calls and meetings").font(theme::bold(24.0)).color(palette.text));
            ui.add_space(18.0);

            // Join by link.
            let link_id = egui::Id::new("chaat-join-link");
            let mut link: String = ui.ctx().data(|d| d.get_temp(link_id)).unwrap_or_default();
            Frame::new()
                .fill(palette.panel)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(16))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(egui::RichText::new("Join a meeting").font(theme::semibold(15.0)).color(palette.text));
                    ui.add_space(6.0);
                    if let Some(account) = widgets::account_picker(ui, app, "Join as") {
                        app.actions.push(Action::SwitchAccount(account));
                    }
                    // Narrow: the Join button goes under the field, which would
                    // otherwise be too short to show its hint.
                    let narrow = ui.available_width() < 320.0;
                    let join_row = |ui: &mut egui::Ui| {
                        let field = egui::TextEdit::singleline(&mut link)
                            .hint_text(if narrow {
                                "Meeting link or ID"
                            } else {
                                "Paste a Teams meeting link or meeting ID"
                            })
                            .desired_width(if narrow { ui.available_width() } else { ui.available_width() - 90.0 })
                            .margin(Margin::symmetric(8, 6));
                        let response = ui.add(field);
                        let valid = crate::calling::is_teams_link(link.trim())
                            || crate::app::App::is_meeting_id(link.trim());
                        let join = ui.add_enabled(valid, egui::Button::new("Join").min_size(vec2(72.0, 30.0)));
                        let enter = response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if valid && (join.clicked() || enter) {
                            app.actions.push(Action::JoinMeeting(link.trim().to_owned()));
                            link.clear();
                        }
                    };
                    if narrow {
                        ui.vertical(join_row);
                    } else {
                        ui.horizontal(join_row);
                    }
                });
            ui.ctx().data_mut(|d| d.insert_temp(link_id, link));
            ui.add_space(18.0);

            // Where calls run, and the way to change it.
            let place = match app.opener() {
                crate::calling::Opener::AppWindow(browser) => format!("Teams on the web, in a {} window beside Chaat", browser.kind.short()),
                crate::calling::Opener::Browser => "Teams on the web, in your default browser".to_owned(),
                crate::calling::Opener::TeamsApp => "the Microsoft Teams app".to_owned(),
            };
            Frame::new()
                .fill(palette.panel)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin::same(16))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(egui::RichText::new("How calls work").font(theme::semibold(15.0)).color(palette.text));
                    ui.add_space(6.0);
                    ui.add(egui::Label::new(egui::RichText::new(format!("Calls and meetings run in Microsoft's own client, not inside Chaat, so they cost nothing extra. They open in {place}; use that window for audio, video and call controls. Chaat shows the meeting chat and follows the call.")).font(theme::regular(13.0)).color(palette.secondary)).wrap());
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(egui::RichText::new("Only Microsoft's clients ring for incoming calls. Keep Teams on the web or Teams mobile signed in to be reachable.").font(theme::regular(12.0)).color(palette.dim)).wrap());
                    ui.add_space(10.0);
                    if ui.add(egui::Button::new("Choose where calls open").min_size(vec2(0.0, 30.0))).clicked() {
                        app.actions.push(Action::ShowSettingsPage(SettingsPage::Calls));
                    }
                });
        });
    });
}
