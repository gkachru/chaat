//! The conversation header: title, people, agents and call buttons.

use super::*;

pub(super) fn header(app: &mut App, ui: &mut egui::Ui, conversation: &ConversationId) {
    let palette = app.palette;
    let me = app.me().to_owned();
    egui::Panel::top("conversation-header")
        .exact_size(theme::HEADER_HEIGHT)
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(palette.window)
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            let rect = ui.max_rect();
            widgets::window_drag(ui, rect, "header-drag");
            ui.painter().hline(
                rect.x_range().expand(16.0),
                rect.bottom(),
                Stroke::new(1.0, palette.outline),
            );
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                let mut call_users: Vec<String> = Vec::new();
                let mut join: Option<String> = None;
                let web: Option<String>;
                // A group chat's people button: chat id and member count.
                let mut people: Option<(String, usize)> = None;
                let (title, subtitle) = match conversation {
                    ConversationId::Chat(id) => {
                        let chat = app.chat(id).cloned().unwrap_or_default();
                        let title = chat.title(&me);
                        call_users = chat
                            .members
                            .iter()
                            .filter(|m| m.user_id != me)
                            .filter_map(|m| m.email.clone())
                            .collect();
                        join = chat.join_url.clone();
                        web = chat.web_url.clone();
                        match chat.kind {
                            ChatKind::OneOnOne => {
                                let peer = chat.face(&me);
                                widgets::avatar(
                                    ui,
                                    app,
                                    peer.map(|p| p.user_id.as_str()),
                                    &title,
                                    32.0,
                                    true,
                                );
                                let subtitle = if chat.is_notes() {
                                    "Notes to yourself. Only you can see them.".to_owned()
                                } else {
                                    peer.and_then(|p| app.presence.get(&p.user_id))
                                        .map(|p| match &p.status_message {
                                            Some(message) => {
                                                format!("{} · {message}", p.describe())
                                            }
                                            None => p.describe(),
                                        })
                                        .or_else(|| peer.and_then(|p| p.email.clone()))
                                        .unwrap_or_default()
                                };
                                (title, subtitle)
                            }
                            ChatKind::Group => {
                                widgets::avatar(ui, app, None, &title, 32.0, false);
                                people = Some((id.clone(), chat.members.len()));
                                (title, format!("{} people", chat.members.len()))
                            }
                            ChatKind::Meeting => {
                                let (tile, _) =
                                    ui.allocate_exact_size(egui::Vec2::splat(32.0), Sense::hover());
                                ui.painter().rect_filled(
                                    tile,
                                    CornerRadius::same(8),
                                    palette.surface_active,
                                );
                                Icon::Calendar.image(palette.accent, 16.0).paint_at(
                                    ui,
                                    egui::Rect::from_center_size(
                                        tile.center(),
                                        egui::Vec2::splat(16.0),
                                    ),
                                );
                                people = Some((id.clone(), chat.members.len()));
                                (title, "Meeting chat".to_owned())
                            }
                        }
                    }
                    ConversationId::Channel { team, channel } => {
                        let channel_data = app.channel(team, channel).cloned();
                        let team_name = app.team(team).map(|t| t.name.clone()).unwrap_or_default();
                        web = channel_data.as_ref().and_then(|c| c.web_url.clone());
                        let (tile, _) =
                            ui.allocate_exact_size(egui::Vec2::splat(32.0), Sense::hover());
                        ui.painter().rect_filled(
                            tile,
                            CornerRadius::same(8),
                            palette.surface_active,
                        );
                        Icon::Hash.image(palette.secondary, 16.0).paint_at(
                            ui,
                            egui::Rect::from_center_size(tile.center(), egui::Vec2::splat(16.0)),
                        );
                        let name = channel_data
                            .as_ref()
                            .map(|c| c.name.clone())
                            .unwrap_or_else(|| "Channel".into());
                        let subtitle = channel_data
                            .and_then(|c| c.description)
                            .map(|d| format!("{team_name} · {d}"))
                            .unwrap_or(team_name);
                        (name, subtitle)
                    }
                };
                // During a call in this chat the call bar has the controls.
                let in_call = app
                    .call
                    .as_ref()
                    .is_some_and(|c| !c.is_ended() && c.chat.as_ref() == Some(conversation));
                if in_call {
                    call_users.clear();
                    join = None;
                }
                // A meeting chat's recordings, from its loaded messages.
                let recordings: Vec<(String, String)> = match conversation {
                    ConversationId::Chat(id)
                        if app.chat(id).is_some_and(|c| c.kind == ChatKind::Meeting) =>
                    {
                        let now = crate::time::now();
                        app.conversations
                            .get(conversation)
                            .map(|view| {
                                recording_messages(&view.messages)
                                    .into_iter()
                                    .filter_map(|m| {
                                        let r = m.recording.as_ref()?;
                                        let line = match &r.duration {
                                            Some(d) => format!(
                                                "{} · {} · {d}",
                                                r.name,
                                                crate::time::day_label(m.created, now)
                                            ),
                                            None => format!(
                                                "{} · {}",
                                                r.name,
                                                crate::time::day_label(m.created, now)
                                            ),
                                        };
                                        Some((line, r.url.clone()))
                                    })
                                    .collect()
                            })
                            .unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                // The title takes what the buttons on the right leave.
                let menu = crate::ui::agents::ReadMenu::of(app, conversation);
                let icons = usize::from(menu != crate::ui::agents::ReadMenu::Hidden)
                    + usize::from(!recordings.is_empty())
                    + usize::from(web.is_some())
                    + if call_users.is_empty() { 0 } else { 2 };
                let mut buttons = icons as f32 * 34.0 + 16.0;
                if join.is_some() {
                    buttons += 80.0;
                }
                // The people button: icon, count and its spacing, or just
                // the icon when the title would be squeezed out.
                let compact = people.is_some()
                    && ui.available_width()
                        - buttons
                        - 64.0
                        - crate::ui::header_right_inset(ui.ctx())
                        < 80.0;
                if people.is_some() {
                    buttons += if compact { 34.0 } else { 64.0 };
                }
                if app
                    .loading
                    .contains(&crate::sync::Resource::Messages(conversation.clone()))
                {
                    buttons += 18.0;
                }
                ui.vertical(|ui| {
                    ui.add_space(8.0);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let width =
                        (ui.available_width() - buttons - crate::ui::header_right_inset(ui.ctx()))
                            .max(48.0);
                    ui.add(egui::Label::new(super::widgets::elided(
                        ui,
                        &title,
                        theme::semibold(15.0),
                        palette.text,
                        width,
                    )));
                    if !subtitle.is_empty() {
                        ui.add(egui::Label::new(super::widgets::elided(
                            ui,
                            &subtitle,
                            theme::regular(12.0),
                            palette.secondary,
                            width,
                        )));
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    // The header's own margin already covers part of the
                    // title bar buttons' room.
                    let buttons = crate::ui::header_right_inset(ui.ctx());
                    if buttons > 0.0 {
                        ui.add_space(buttons - 8.0);
                    }
                    if menu != crate::ui::agents::ReadMenu::Hidden {
                        agents_menu(ui, &palette, conversation, menu, &mut app.actions);
                    }
                    if !recordings.is_empty() {
                        recordings_menu(ui, &palette, conversation, &recordings, &mut app.actions);
                    }
                    if let Some(web) = web {
                        let open = widgets::icon_button(
                            ui,
                            &palette,
                            Icon::ExternalLink,
                            "Open in Microsoft Teams",
                        );
                        widgets::on_click(&open, &mut app.actions, Action::OpenUrl(web));
                    }
                    if !call_users.is_empty() {
                        let audio = widgets::icon_button(ui, &palette, Icon::Phone, "Audio call");
                        widgets::on_click(
                            &audio,
                            &mut app.actions,
                            Action::StartCall {
                                users: call_users.clone(),
                                video: false,
                            },
                        );
                        let video = widgets::icon_button(ui, &palette, Icon::Video, "Video call");
                        widgets::on_click(
                            &video,
                            &mut app.actions,
                            Action::StartCall {
                                users: call_users,
                                video: true,
                            },
                        );
                    }
                    if let Some((chat, count)) = people {
                        people_button(ui, app, &chat, count, compact);
                    }
                    if let Some(join) = join {
                        ui.add_space(4.0);
                        let response = widgets::pill(ui, &palette, Icon::Video, "Join", true)
                            .on_hover_text("Join the meeting");
                        widgets::on_click(&response, &mut app.actions, Action::JoinMeeting(join));
                    }
                    if app
                        .loading
                        .contains(&crate::sync::Resource::Messages(conversation.clone()))
                    {
                        widgets::loading(ui, palette.dim, 14.0);
                    }
                });
            });
        });
}

/// A meeting chat's recordings: one row each, opening it in the browser.
fn recordings_menu(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    conversation: &ConversationId,
    recordings: &[(String, String)],
    actions: &mut Vec<Action>,
) {
    let button = widgets::icon_button(ui, palette, Icon::Play, "Recordings");
    let id = match conversation {
        ConversationId::Chat(chat) => egui::Id::new(("recordings-menu", chat)),
        ConversationId::Channel { .. } => egui::Id::new("recordings-menu"),
    };
    egui::Popup::menu(&button).id(id).show(|ui| {
        ui.set_min_width(240.0);
        for (line, url) in recordings {
            if widgets::menu_item(ui, palette, Icon::Play, line) {
                actions.push(Action::OpenUrl(url.clone()));
            }
        }
    });
}

/// The header's More menu, shown while an agents plugin is on: let agents
/// read this conversation, or stop them.
fn agents_menu(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    conversation: &ConversationId,
    menu: crate::ui::agents::ReadMenu,
    actions: &mut Vec<Action>,
) {
    let more = widgets::icon_button(ui, palette, Icon::More, "More");
    egui::Popup::menu(&more)
        .id(crate::ui::agents::menu_id(conversation))
        .show(|ui| {
            ui.set_min_width(200.0);
            match crate::ui::agents::menu_label(menu, conversation) {
                Some(label) => {
                    let on = menu == crate::ui::agents::ReadMenu::Let;
                    let icon = if on { Icon::Plus } else { Icon::Close };
                    if widgets::menu_item(ui, palette, icon, label) {
                        actions.push(Action::Agent(AgentAction::LetRead {
                            conversation: conversation.clone(),
                            on,
                        }));
                    }
                }
                None => {
                    ui.label(
                        egui::RichText::new("Agents may read all of these")
                            .font(theme::regular(12.0))
                            .color(palette.dim),
                    );
                }
            }
        });
}

/// The header's people button of a group chat and the panel it opens: the
/// members, Add people and Leave.
fn people_button(ui: &mut egui::Ui, app: &mut App, chat: &str, count: usize, compact: bool) {
    let palette = app.palette;
    let id = egui::Id::new(("people-panel", chat));
    let was_open = egui::Popup::is_id_open(ui.ctx(), id);
    let button = if compact {
        widgets::icon_button(ui, &palette, Icon::Teams, &format!("People ({count})"))
    } else {
        widgets::pill(ui, &palette, Icon::Teams, &count.to_string(), false).on_hover_text("People")
    };
    if button.clicked() && !was_open {
        app.actions.push(Action::PeopleOpened(chat.to_owned()));
    }
    let members = app.group_members(chat);
    let me = app.me().to_owned();
    let mut actions = Vec::new();
    egui::Popup::menu(&button).id(id).width(280.0).show(|ui| {
        ui.set_width(280.0);
        ui.label(
            egui::RichText::new(format!("People ({})", members.len()))
                .font(theme::semibold(14.0))
                .color(palette.text),
        );
        ui.add_space(4.0);
        egui::ScrollArea::vertical()
            .max_height(360.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for member in &members {
                    let mine = member.user_id == me;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 8.0;
                        let face = widgets::avatar(
                            ui,
                            app,
                            Some(&member.user_id),
                            &member.display_name,
                            28.0,
                            true,
                        );
                        // Presence of the signed-in person is not in the
                        // followed-people map.
                        if mine
                            && !app.presence.contains_key(&member.user_id)
                            && let Some(state) = &app.my_presence
                        {
                            let rect = face.rect;
                            let dot = 9.0;
                            let center = rect.right_bottom() - vec2(dot * 0.42, dot * 0.42);
                            ui.painter()
                                .circle_filled(center, dot / 2.0 + 1.5, palette.panel);
                            ui.painter().circle_filled(
                                center,
                                dot / 2.0,
                                palette.presence(state.availability),
                            );
                        }
                        let name = if mine { "You" } else { &member.display_name };
                        let activity = (!mine)
                            .then(|| app.presence.get(&member.user_id))
                            .flatten()
                            .filter(|p| p.activity.is_some())
                            .map(|p| p.describe());
                        let name = egui::RichText::new(name)
                            .font(theme::regular(13.0))
                            .color(palette.text);
                        if let Some(activity) = activity {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.label(name);
                                ui.label(
                                    egui::RichText::new(activity)
                                        .font(theme::regular(12.0))
                                        .color(palette.dim),
                                );
                            });
                        } else {
                            // Centered on the avatar, as before activity lines.
                            ui.label(name);
                        }
                        if app
                            .adding
                            .contains(&(chat.to_owned(), member.user_id.clone()))
                        {
                            ui.label(
                                egui::RichText::new("Adding…")
                                    .font(theme::regular(12.0))
                                    .color(palette.dim),
                            );
                        }
                    });
                }
            });
        ui.separator();
        if widgets::menu_item(ui, &palette, Icon::UserPlus, "Add people") {
            actions.push(Action::OpenAddPeople(chat.to_owned()));
        }
        if widgets::menu_item(ui, &palette, Icon::LogOut, "Leave") {
            actions.push(Action::OpenLeaveChat(chat.to_owned()));
        }
    });
    app.actions.extend(actions);
}
