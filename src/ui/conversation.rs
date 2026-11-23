//! The open chat or channel: header, virtualized history, composer.
//!
//! The history lays out only the rows near the viewport. Every other row
//! takes the height it had when last measured (or an estimate), so a
//! conversation with thousands of messages costs what its visible rows cost.

use egui::{Align, CornerRadius, Frame, Key, Layout, Margin, Modifiers, Sense, Stroke, vec2};

use super::{composer_tools, find, rich, widgets};
use crate::app::{App, Draft};
use crate::model::{
    Access, Action, AttachmentKind, ChatKind, ConversationId, Delivery, Message,
    QUICK_REACTION_NAMES, QUICK_REACTIONS,
};
use crate::theme::{self, Icon};

/// Room for the avatar left of other people's messages.
const GUTTER: f32 = 44.0;
const AVATAR: f32 = 32.0;
/// Between a missed call's words and its Call back button.
const CALL_BACK_GAP: f32 = 10.0;
/// How long the pointer rests on a message before its hover bar shows.
const HOVER_DELAY: f64 = 0.3;
/// Height of the name and time line above a message group.
const HEADER: f32 = 19.0;
/// Space between a bubble's edge and its text.
const BUBBLE_PAD: Margin = Margin {
    left: 12,
    right: 12,
    top: 7,
    bottom: 8,
};

/// The widest a chat bubble gets, and the width its text wraps at.
fn bubble_widths(width: f32) -> (f32, f32) {
    let bubble = ((width - GUTTER - 8.0) * 0.85).clamp(120.0, 760.0);
    let text = bubble - f32::from(BUBBLE_PAD.left + BUBBLE_PAD.right);
    (bubble, text)
}

pub fn show(app: &mut App, ui: &mut egui::Ui, conversation: ConversationId) {
    header(app, ui, &conversation);
    find::show(app, ui, &conversation);
    pin_banner(app, ui, &conversation);
    composer(app, ui, &conversation);
    history(app, ui, &conversation);
}

// -- Pin banner -----------------------------------------------------------------

/// The icon of pinned messages.
const PIN_ICON: Icon = Icon::Pin;

/// The pin shown after stepping `delta` (1 or -1) from `index` among `len`.
fn step_pin(index: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    (index.min(len - 1) as isize + delta).rem_euclid(len as isize) as usize
}

fn banner_bottom_id() -> egui::Id {
    egui::Id::new("pin-banner-bottom")
}

/// The bottom edge of the pin banner when this frame drew one. Cards that
/// float over the conversation's top right start below it, so they never
/// cover its controls.
pub fn pin_banner_bottom(ctx: &egui::Context) -> Option<f32> {
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|d| d.get_temp::<(u64, f32)>(banner_bottom_id()))
        .filter(|(drawn, _)| *drawn == pass)
        .map(|(_, bottom)| bottom)
}

/// One line under the header for the open chat's pinned messages: the
/// current pin, arrows to step through them, and Unpin.
fn pin_banner(app: &mut App, ui: &mut egui::Ui, conversation: &ConversationId) {
    let ConversationId::Chat(chat) = conversation else {
        return;
    };
    let pins = app.pins.get(chat).cloned().unwrap_or_default();
    if pins.is_empty() {
        return;
    }
    let palette = app.palette;
    let state = egui::Id::new(("pin-index", chat));
    let index = ui
        .data(|d| d.get_temp::<usize>(state))
        .unwrap_or(0)
        .min(pins.len() - 1);
    let pin = &pins[index];
    let mut actions = Vec::new();
    let mut next_index = index;
    let banner = egui::Panel::top(egui::Id::new(("pin-banner", chat)))
        .exact_size(34.0)
        .show_separator_line(false)
        .frame(
            Frame::new()
                .fill(palette.window)
                .inner_margin(Margin::symmetric(16, 0)),
        )
        .show(ui, |ui| {
            let rect = ui.max_rect();
            ui.painter().hline(
                rect.x_range().expand(16.0),
                rect.bottom(),
                Stroke::new(1.0, palette.outline),
            );
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let (icon, _) = ui.allocate_exact_size(vec2(14.0, 14.0), Sense::hover());
                PIN_ICON.image(palette.secondary, 14.0).paint_at(ui, icon);
                let mut jump = false;
                // Right to left: Unpin, arrows and count, then the text takes
                // the rest.
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if widgets::text_button(ui, &palette, "Unpin", theme::regular(12.0)).clicked() {
                        actions.push(Action::UnpinMessage {
                            chat: chat.clone(),
                            pin: pin.id.clone(),
                        });
                    }
                    if pins.len() > 1 {
                        let next = widgets::icon_button_sized(
                            ui,
                            &palette,
                            Icon::ChevronRight,
                            "Next pinned message",
                            24.0,
                            14.0,
                            palette.secondary,
                        );
                        if next.clicked() {
                            next_index = step_pin(index, pins.len(), 1);
                        }
                        ui.label(
                            egui::RichText::new(format!("{} of {}", index + 1, pins.len()))
                                .font(theme::regular(12.0))
                                .color(palette.dim),
                        );
                        let prev = widgets::icon_button_sized(
                            ui,
                            &palette,
                            Icon::ChevronLeft,
                            "Previous pinned message",
                            24.0,
                            14.0,
                            palette.secondary,
                        );
                        if prev.clicked() {
                            next_index = step_pin(index, pins.len(), -1);
                        }
                    }
                    // Author and preview as one label truncated to the room
                    // the controls leave, so it never runs under them. (A
                    // horizontal layout would make egui ignore the job's own
                    // truncation.)
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        let mut job = egui::text::LayoutJob::default();
                        let format = |font, color| egui::TextFormat {
                            font_id: font,
                            color,
                            ..Default::default()
                        };
                        // With little room the author would take all of it and
                        // hide the message; then only the message shows.
                        let author_width = ui
                            .painter()
                            .layout_no_wrap(pin.author.clone(), theme::semibold(12.0), palette.text)
                            .size()
                            .x;
                        let with_author = ui.available_width() >= author_width + 6.0 + 90.0;
                        if with_author {
                            job.append(
                                &pin.author,
                                0.0,
                                format(theme::semibold(12.0), palette.text),
                            );
                        }
                        job.append(
                            &pin.preview.replace('\n', " "),
                            if with_author { 6.0 } else { 0.0 },
                            format(theme::regular(12.0), palette.secondary),
                        );
                        let text = ui
                            .add(
                                egui::Label::new(job)
                                    .truncate()
                                    .selectable(false)
                                    .sense(Sense::click()),
                            )
                            .on_hover_text(format!("{}: {}", pin.author, pin.preview))
                            .on_hover_cursor(egui::CursorIcon::PointingHand);
                        jump = text.clicked();
                    });
                });
                if jump {
                    actions.push(Action::OpenMessage {
                        conversation: conversation.clone(),
                        message: pin.message_id.clone(),
                    });
                }
            });
        });
    let pass = ui.ctx().cumulative_pass_nr();
    ui.data_mut(|d| d.insert_temp(banner_bottom_id(), (pass, banner.response.rect.bottom())));
    if next_index != index {
        ui.data_mut(|d| d.insert_temp(state, next_index));
    }
    app.actions.extend(actions);
}

// -- Header -------------------------------------------------------------------

fn header(app: &mut App, ui: &mut egui::Ui, conversation: &ConversationId) {
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
                // The title takes what the buttons on the right leave.
                let menu = super::agents::ReadMenu::of(app, conversation);
                let icons = usize::from(menu != super::agents::ReadMenu::Hidden)
                    + usize::from(web.is_some())
                    + if call_users.is_empty() { 0 } else { 2 };
                let mut buttons = icons as f32 * 34.0 + 16.0;
                if join.is_some() {
                    buttons += 80.0;
                }
                // The people button: icon, count and its spacing, or just
                // the icon when the title would be squeezed out.
                let compact = people.is_some()
                    && ui.available_width() - buttons - 64.0 - super::header_right_inset(ui.ctx())
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
                        (ui.available_width() - buttons - super::header_right_inset(ui.ctx()))
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
                    let buttons = super::header_right_inset(ui.ctx());
                    if buttons > 0.0 {
                        ui.add_space(buttons - 8.0);
                    }
                    if menu != super::agents::ReadMenu::Hidden {
                        agents_menu(ui, &palette, conversation, menu, &mut app.actions);
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

/// The header's More menu, shown while an agents plugin is on: let agents
/// read this conversation, or stop them.
fn agents_menu(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    conversation: &ConversationId,
    menu: super::agents::ReadMenu,
    actions: &mut Vec<Action>,
) {
    let more = widgets::icon_button(ui, palette, Icon::More, "More");
    egui::Popup::menu(&more)
        .id(super::agents::menu_id(conversation))
        .show(|ui| {
            ui.set_min_width(200.0);
            match super::agents::menu_label(menu, conversation) {
                Some(label) => {
                    let on = menu == super::agents::ReadMenu::Let;
                    let icon = if on { Icon::Plus } else { Icon::Close };
                    if widgets::menu_item(ui, palette, icon, label) {
                        actions.push(Action::LetAgentsRead {
                            conversation: conversation.clone(),
                            on,
                        });
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

// -- History ------------------------------------------------------------------

/// A guess at a row's height before it is laid out.
fn estimate(message: &Message, width: f32, grouped: bool) -> f32 {
    let chars = message.body.to_plain().chars().count() as f32;
    let per_line = (bubble_widths(width).1 / 7.2).max(20.0);
    let lines = (chars / per_line).ceil().max(1.0)
        + message.body.blocks.len().saturating_sub(1) as f32 * 0.4;
    let mut height = lines * 20.0 + 18.0;
    if !grouped {
        height += HEADER + 10.0;
    }
    if !message.reactions.is_empty() {
        height += 34.0;
    }
    if message.quote().is_some() {
        height += 54.0;
    }
    height += message.body.images().count() as f32 * 180.0;
    for attachment in &message.attachments {
        if attachment.kind == AttachmentKind::Card {
            height += match attachment.content.as_deref().and_then(crate::cards::parse) {
                Some(card) => {
                    40.0 + 24.0 * card.body.len() as f32 + 32.0 * card.actions.len() as f32
                }
                None => 24.0,
            };
        }
    }
    height
}

fn history(app: &mut App, ui: &mut egui::Ui, conversation: &ConversationId) {
    let palette = app.palette;
    let channel = conversation.is_channel();
    let empty = app
        .conversation(conversation)
        .is_none_or(|c| c.messages.is_empty());
    if empty {
        egui::CentralPanel::no_frame().show(ui, |ui| {
            if channel && matches!(app.channel_access, Access::NeedsConsent | Access::Forbidden) {
                channel_consent(app, ui, conversation);
            } else if app
                .loading
                .contains(&crate::sync::Resource::Messages(conversation.clone()))
            {
                widgets::empty_state(ui, &palette, Icon::Loader, "Loading messages…", "");
            } else {
                widgets::empty_state(
                    ui,
                    &palette,
                    Icon::Chat,
                    "No messages yet",
                    "Say hello below.",
                );
            }
        });
        return;
    }
    let me = app.me().to_owned();
    let now = crate::time::now();
    let mut actions: Vec<Action> = Vec::new();
    let jump = app.jump_to.take();
    let hover_id = egui::Id::new(("chaat-hover-row", conversation.key()));
    let previous_hover: Option<(String, egui::Rect)> = ui.ctx().data(|d| d.get_temp(hover_id));
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let menu_id = egui::Id::new(("chaat-message-menu", conversation.key()));
    let menu_open = egui::Popup::is_id_open(ui.ctx(), menu_id)
        || egui::Popup::is_id_open(ui.ctx(), menu_id.with("react-popup"));
    let mut hovered: Option<(usize, egui::Rect, egui::Rect, bool)> = None;
    let scroll_to_end = app.scroll_to_end > 0;
    app.scroll_to_end = app.scroll_to_end.saturating_sub(1);

    egui::CentralPanel::no_frame().show(ui, |ui| {
        let Some(view) = app.conversations.get_mut(conversation) else {
            return;
        };
        let more = view.more;
        let mut restore = std::mem::take(&mut view.restore_history_anchor)
            .then(|| view.history_anchor.clone())
            .flatten();
        let previous_anchor = view.history_anchor.clone();
        let at_end_id = egui::Id::new(("chaat-history-at-end", conversation.key()));
        let at_end = ui.ctx().data(|d| d.get_temp(at_end_id)).unwrap_or(true);
        let mut anchor = None;
        let mut heights = std::mem::take(&mut view.heights);
        let mut layout = std::mem::take(&mut view.layout);
        let expanded = view.expanded.clone();
        let divider = if channel {
            None
        } else {
            view.place_divider(&me)
        };
        let messages = std::mem::take(&mut view.messages);
        let app_ro: &App = app;
        // Opening a conversation shows its newest messages; from then on egui
        // keeps the view at the end while it is there.
        let output = egui::ScrollArea::vertical()
            .id_salt(("history", conversation.key()))
            .auto_shrink([false, false])
            .stick_to_bottom(jump.is_none())
            .show(ui, |ui| {
                let clip = ui.clip_rect();
                let scroll_spacing = ui.spacing().item_spacing.y;
                let margin = (clip.height() * 2.0).max(600.0);
                Frame::new()
                    .inner_margin(Margin {
                        left: 16,
                        right: 20,
                        top: 12,
                        bottom: 12,
                    })
                    .show(ui, |ui| {
                        let width = ui.available_width();
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        // A new width measures every row again, and the
                        // estimates above the view would move it. Away
                        // from the end, keep the first visible row where
                        // it was; at the end, egui keeps the end.
                        if restore.is_none()
                            && !at_end
                            && layout.resized(width, theme::font_revision(ui.ctx()))
                        {
                            restore = previous_anchor.clone();
                        }
                        if more {
                            ui.vertical_centered(|ui| {
                                let response = widgets::secondary_button(
                                    ui,
                                    &palette,
                                    "Load earlier messages",
                                );
                                widgets::on_click(
                                    &response,
                                    &mut actions,
                                    Action::LoadOlder(conversation.clone()),
                                );
                                // Reaching the top loads more on its own.
                                if ui.is_rect_visible(response.rect)
                                    && !app_ro.loading.contains(&crate::sync::Resource::Messages(
                                        conversation.clone(),
                                    ))
                                    && ui.input(|i| i.smooth_scroll_delta.y > 0.0)
                                {
                                    actions.push(Action::LoadOlder(conversation.clone()));
                                }
                            });
                            ui.add_space(12.0);
                        }
                        layout.prepare(
                            &messages,
                            width,
                            theme::font_revision(ui.ctx()),
                            channel,
                            &mut heights,
                            |message, grouped| estimate(message, width, grouped),
                        );
                        let origin = ui.cursor().top();
                        let visible = layout.visible(
                            clip.top() - origin - margin,
                            clip.bottom() - origin + margin,
                        );
                        let target_id =
                            jump.as_ref().or_else(|| restore.as_ref().map(|(id, _)| id));
                        let target_index =
                            target_id.and_then(|id| messages.iter().position(|m| &m.id == id));
                        let mut ranges = vec![visible];
                        if let Some(index) = target_index {
                            ranges.push(index..index + 1);
                        }
                        ranges.sort_by_key(|range| range.start);
                        let mut end = 0;
                        for range in ranges {
                            let start = range.start.max(end);
                            if start >= range.end {
                                continue;
                            }
                            let skipped = layout.prefix(start) - (ui.cursor().top() - origin);
                            if skipped > 0.0 {
                                ui.add_space(skipped);
                            }
                            for (index, message) in
                                messages.iter().enumerate().take(range.end).skip(start)
                            {
                                let (new_day, grouped) = layout.flags(index);
                                if new_day {
                                    day_divider(ui, app_ro, message.created, now);
                                }
                                let top = ui.cursor().top();
                                let target = target_index == Some(index);
                                // The divider is part of the row's measured
                                // height, and starts a new group.
                                let grouped = if divider == Some(index) {
                                    unread_divider(ui, &palette);
                                    false
                                } else {
                                    grouped
                                };
                                let (rect, bubble) = if channel {
                                    let rect = thread_card(
                                        ui,
                                        app_ro,
                                        conversation,
                                        message,
                                        &me,
                                        now,
                                        expanded.contains(&message.id),
                                        &mut actions,
                                    );
                                    (rect, None)
                                } else {
                                    let (rect, bubble) = message_row(
                                        ui,
                                        app_ro,
                                        conversation,
                                        message,
                                        &me,
                                        grouped,
                                        target,
                                        width,
                                        &mut actions,
                                    );
                                    (rect, Some(bubble))
                                };
                                let measured = ui.cursor().top() - top;
                                heights.insert(message.id.clone(), measured);
                                layout.measure(index, measured);
                                if jump.as_deref() == Some(message.id.as_str()) {
                                    ui.scroll_to_rect(rect, Some(Align::Center));
                                }
                                if let Some((id, offset)) = &restore
                                    && id == &message.id
                                {
                                    ui.scroll_to_rect_animation(
                                        egui::Rect::from_min_size(
                                            // ScrollArea adds item spacing to an
                                            // aligned target; cancel that here.
                                            rect.min - vec2(0.0, *offset - scroll_spacing),
                                            vec2(1.0, clip.height()),
                                        ),
                                        Some(Align::TOP),
                                        egui::style::ScrollAnimation::none(),
                                    );
                                }
                                if anchor.is_none()
                                    && rect.bottom() >= clip.top()
                                    && rect.top() <= clip.bottom()
                                {
                                    anchor = Some((message.id.clone(), rect.top() - clip.top()));
                                }
                                if !message.system && !message.deleted {
                                    // The toolbar, and its menu while open,
                                    // keep their message hovered.
                                    let keep =
                                        previous_hover.as_ref().is_some_and(|(id, toolbar)| {
                                            id == &message.id
                                                && (menu_open
                                                    || pointer.is_some_and(|p| toolbar.contains(p)))
                                        });
                                    let kept = hovered.is_some_and(|(i, ..)| {
                                        menu_open
                                            && previous_hover
                                                .as_ref()
                                                .is_some_and(|(id, _)| id == &messages[i].id)
                                    });
                                    if keep
                                        || (!kept
                                            && pointer.is_some_and(|p| {
                                                // In chats only the bubble itself.
                                                bubble.unwrap_or(rect).contains(p)
                                                    && clip.contains(p)
                                            }))
                                    {
                                        hovered = Some((
                                            index,
                                            rect,
                                            bubble.unwrap_or(rect),
                                            message.is_from(&me),
                                        ));
                                    }
                                }
                            }
                            end = range.end;
                        }
                        let remaining = layout.total() - (ui.cursor().top() - origin);
                        if remaining > 0.0 {
                            ui.add_space(remaining);
                        }
                        ui.add_space(4.0);
                        if scroll_to_end && jump.is_none() {
                            let end = egui::Rect::from_min_size(ui.cursor().min, vec2(1.0, 1.0));
                            ui.scroll_to_rect_animation(
                                end,
                                Some(Align::BOTTOM),
                                egui::style::ScrollAnimation::none(),
                            );
                        }
                        // The bar waits until the pointer rests on a
                        // message, so scrolling past bubbles shows nothing.
                        // A bar already showing stays.
                        let since_id = egui::Id::new(("chaat-hover-since", conversation.key()));
                        let now = ui.input(|i| i.time);
                        let scrolling = ui.input(|i| i.smooth_scroll_delta.y != 0.0);
                        let hovered = hovered.filter(|(index, ..)| {
                            let id = &messages[*index].id;
                            if !scrolling
                                && previous_hover
                                    .as_ref()
                                    .is_some_and(|(shown, _)| shown == id)
                            {
                                return true;
                            }
                            let since: Option<(String, f64)> =
                                ui.ctx().data(|d| d.get_temp(since_id));
                            match since {
                                Some((resting, at)) if &resting == id && !scrolling => {
                                    let waited = now - at;
                                    if waited < HOVER_DELAY {
                                        ui.ctx().request_repaint_after(
                                            std::time::Duration::from_secs_f64(
                                                HOVER_DELAY - waited,
                                            ),
                                        );
                                    }
                                    waited >= HOVER_DELAY
                                }
                                _ => {
                                    ui.ctx().data_mut(|d| {
                                        d.insert_temp(since_id, (id.clone(), now));
                                    });
                                    ui.ctx().request_repaint_after(
                                        std::time::Duration::from_secs_f64(HOVER_DELAY),
                                    );
                                    false
                                }
                            }
                        });
                        if let Some((index, rect, anchor, mine)) = &hovered {
                            let message = &messages[*index];
                            let id = &message.id;
                            let toolbar = hover_toolbar(
                                ui,
                                app_ro,
                                conversation,
                                id,
                                Some(message),
                                Placement {
                                    row: *rect,
                                    bubble: (!channel).then_some(*anchor),
                                },
                                *mine,
                                menu_id,
                                &mut actions,
                            );
                            ui.ctx()
                                .data_mut(|d| d.insert_temp(hover_id, (id.clone(), toolbar)));
                        } else {
                            ui.ctx()
                                .data_mut(|d| d.remove::<(String, egui::Rect)>(hover_id));
                        }
                    });
            });
        // Whether the view sits at the end, for the next frame.
        let at_end =
            output.state.offset.y + output.inner_rect.height() >= output.content_size.y - 2.0;
        ui.ctx().data_mut(|d| d.insert_temp(at_end_id, at_end));
        let far = find::jump_visible(
            output.content_size.y,
            output.state.offset.y,
            output.inner_rect.height(),
        );
        find::jump_button(
            ui,
            &palette,
            conversation,
            output.inner_rect,
            far,
            &mut actions,
        );
        if let Some(view) = app.conversations.get_mut(conversation) {
            if view.messages.is_empty() {
                view.messages = messages;
            } else {
                // Messages arrived while drawing (they cannot: actions run
                // after the frame), keep both to be safe.
                let mut merged = messages;
                merged.append(&mut view.messages);
                view.messages = merged;
            }
            view.heights = heights;
            view.layout = layout;
            if restore.is_none() {
                view.history_anchor = anchor;
            }
        }
    });
    app.actions.extend(actions);
}

fn day_divider(ui: &mut egui::Ui, app: &App, at: i64, now: i64) {
    let palette = &app.palette;
    ui.add_space(10.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 22.0), Sense::hover());
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        crate::time::day_label(at, now),
        theme::regular(12.0),
        palette.secondary,
    );
    ui.add_space(6.0);
}

/// "Last read" over a line, before the first message the person had not
/// seen when they opened the chat.
fn unread_divider(ui: &mut egui::Ui, palette: &theme::Palette) {
    ui.add_space(12.0);
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(vec2(width, 20.0), Sense::hover());
    let galley = ui
        .ctx()
        .fonts_mut(|f| f.layout_no_wrap("Last read".into(), theme::semibold(12.0), palette.accent));
    let center = rect.center();
    let half = galley.size().x / 2.0 + 16.0;
    let stroke = Stroke::new(1.0, palette.outline);
    ui.painter()
        .hline(rect.left() + GUTTER..=center.x - half, center.y, stroke);
    ui.painter()
        .hline(center.x + half..=rect.right(), center.y, stroke);
    ui.painter()
        .galley(center - galley.size() / 2.0, galley, palette.accent);
    ui.add_space(2.0);
}

/// A system event's icon and words, as Teams shows them: people added or
/// removed by name, and calls with their time and, in bold, how long they
/// lasted. Names Graph left out are looked up.
fn system_line(
    app: &App,
    message: &Message,
    wrap: f32,
    actions: &mut Vec<Action>,
) -> (Option<Icon>, egui::text::LayoutJob) {
    use crate::model::MemberChange;
    let palette = &app.palette;
    let format = |font: egui::FontId, color: egui::Color32| egui::TextFormat {
        font_id: font,
        color,
        valign: Align::Center,
        ..Default::default()
    };
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = wrap;
    let words = format(theme::regular(13.0), palette.secondary);
    if let Some(membership) = &message.membership {
        let unknown: Vec<String> = membership
            .unnamed()
            .filter(|id| app.needs_name(id))
            .map(str::to_owned)
            .collect();
        if !unknown.is_empty() {
            actions.push(Action::LookUpNames(unknown));
        }
        job.append(&membership.describe(|id| app.person_name(id)), 0.0, words);
        let icon = match membership.change {
            MemberChange::Added | MemberChange::Joined => Icon::UserPlus,
            MemberChange::Removed | MemberChange::Left => Icon::UserMinus,
        };
        return (Some(icon), job);
    }
    if let Some(call) = &message.call {
        let now = crate::time::now();
        let clock = crate::time::clock(message.created);
        let at = if crate::time::day_key(message.created) == crate::time::day_key(now) {
            clock
        } else {
            format!("{} {clock}", crate::time::day_label(message.created, now))
        };
        job.append(&at, 0.0, format(theme::regular(12.0), palette.dim));
        let what = if call.meeting { "Meeting" } else { "Call" };
        let words_text = if message.missed_call {
            "Missed call".to_owned()
        } else if call.ended {
            format!("{what} ended")
        } else {
            format!("{what} started")
        };
        job.append(&words_text, 12.0, words.clone());
        if let Some(duration) = call.duration.as_deref().filter(|_| !message.missed_call) {
            job.append(duration, 6.0, format(theme::semibold(13.0), palette.text));
        }
        let icon = if call.meeting {
            Icon::Video
        } else {
            Icon::Phone
        };
        return (Some(icon), job);
    }
    job.append(&message.body.to_plain(), 0.0, words);
    (None, job)
}

/// Whom "Call back" on a missed call rings: everyone else in the chat, as
/// the chat header's call buttons do. Empty outside chats or without
/// addresses, which hides the button.
fn missed_call_back(app: &App, conversation: &ConversationId, me: &str) -> Vec<String> {
    let ConversationId::Chat(id) = conversation else {
        return Vec::new();
    };
    app.chat(id).map_or_else(Vec::new, |chat| {
        chat.members
            .iter()
            .filter(|m| m.user_id != me)
            .filter_map(|m| m.email.clone())
            .collect()
    })
}

/// One chat message as a bubble: on the right for the person's own
/// messages, on the left under the author's name and picture for everyone
/// else's. Returns the row's rect and the bubble's.
#[allow(clippy::too_many_arguments)]
fn message_row(
    ui: &mut egui::Ui,
    app: &App,
    conversation: &ConversationId,
    message: &Message,
    me: &str,
    grouped: bool,
    highlighted: bool,
    width: f32,
    actions: &mut Vec<Action>,
) -> (egui::Rect, egui::Rect) {
    let palette = &app.palette;
    if message.system {
        let call_back = if message.missed_call {
            missed_call_back(app, conversation, me)
        } else {
            Vec::new()
        };
        // Leave room for Call back, so a long date (older than yesterday)
        // wraps instead of pushing the button out of the view.
        let button_width = if call_back.is_empty() {
            0.0
        } else {
            CALL_BACK_GAP + widgets::pill_width(ui, "Call back")
        };
        let text_width = (width - GUTTER - 8.0 - button_width).max(80.0);
        let (icon, job) = system_line(app, message, text_width, actions);
        let response = ui.allocate_ui(vec2(width, 0.0), |ui| {
            ui.set_width(width);
            ui.add_space(6.0);
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                // The icon sits in the avatar column, the words where
                // messages start.
                let (gutter, _) = ui.allocate_exact_size(vec2(GUTTER, 20.0), Sense::hover());
                if let Some(icon) = icon {
                    icon.image(palette.dim, 18.0).paint_at(
                        ui,
                        egui::Rect::from_min_size(
                            gutter.min + vec2(7.0, 1.0),
                            egui::Vec2::splat(18.0),
                        ),
                    );
                }
                let galley = ui.ctx().fonts_mut(|f| f.layout_job(job));
                ui.add(egui::Label::new(galley))
                    .on_hover_text(crate::time::day_label(message.created, crate::time::now()));
                if !call_back.is_empty() {
                    ui.add_space(CALL_BACK_GAP);
                    let response = widgets::pill(ui, palette, Icon::Phone, "Call back", false)
                        .on_hover_text("Call back in Microsoft Teams");
                    widgets::on_click(
                        &response,
                        actions,
                        Action::StartCall {
                            users: call_back,
                            video: false,
                        },
                    );
                }
            });
            ui.add_space(6.0);
        });
        let rect = response.response.rect;
        return (rect, rect);
    }
    let mentioned = message.body.blocks.iter().any(|block| match block {
        crate::html::Block::Paragraph(spans) => spans.iter().any(|s| s.style.mention),
        _ => false,
    }) && app
        .profile
        .as_ref()
        .is_some_and(|p| message.body.to_plain().contains(&p.display_name));
    let mine = message.is_from(me);
    let stroke = if message.importance_high {
        Stroke::new(1.0, palette.danger)
    } else if mentioned {
        Stroke::new(1.0, palette.accent)
    } else {
        Stroke::NONE
    };
    let fill = if mine {
        palette.bubble_mine()
    } else {
        palette.bubble()
    };
    let (bubble_max, text_width) = bubble_widths(width);
    let start = ui.cursor().min;
    let behind = ui.painter().add(egui::Shape::Noop);
    let mut bubble = egui::Rect::NOTHING;
    let response = ui.allocate_ui(vec2(width, 0.0), |ui| {
        ui.set_width(width);
        ui.add_space(if grouped { 2.0 } else { 10.0 });
        ui.spacing_mut().item_spacing.y = 4.0;
        if mine {
            if !grouped {
                let (line, _) = ui.allocate_exact_size(vec2(width, HEADER), Sense::hover());
                header_line(ui, palette, message, line, true);
            }
            bubble = right_aligned(
                ui,
                egui::Id::new(("chaat-bubble", &message.id)),
                bubble_max,
                |ui| {
                    bubble_frame(
                        ui,
                        app,
                        conversation,
                        message,
                        (fill, stroke),
                        text_width,
                        actions,
                    )
                },
            );
            if !message.reactions.is_empty() {
                // The pills tuck 1 point into the bubble's bottom edge.
                ui.add_space(-(ui.spacing().item_spacing.y + REACTION_OVERLAP));
                right_aligned(
                    ui,
                    egui::Id::new(("chaat-reactions", &message.id)),
                    bubble_max,
                    |ui| reactions(ui, app, conversation, message, me, actions),
                );
            }
            let pills = super::agents::pending_reactions(app, conversation, &message.id);
            if !pills.is_empty() {
                right_aligned(
                    ui,
                    egui::Id::new(("chaat-agent-pills", &message.id)),
                    bubble_max,
                    |ui| super::agents::reaction_pills(ui, app, &pills, actions),
                );
            }
        } else {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                let (gutter, _) = ui.allocate_exact_size(
                    vec2(GUTTER, if grouped { 1.0 } else { HEADER + AVATAR }),
                    Sense::hover(),
                );
                if !grouped {
                    let mut child =
                        ui.new_child(egui::UiBuilder::new().max_rect(egui::Rect::from_min_size(
                            gutter.min + vec2(0.0, HEADER + 2.0),
                            egui::Vec2::splat(AVATAR),
                        )));
                    widgets::avatar(
                        &mut child,
                        app,
                        message.author_id.as_deref(),
                        &message.author,
                        AVATAR,
                        true,
                    );
                }
                ui.vertical(|ui| {
                    ui.set_max_width(bubble_max);
                    ui.spacing_mut().item_spacing.y = 4.0;
                    if !grouped {
                        let (line, _) =
                            ui.allocate_exact_size(vec2(bubble_max, HEADER), Sense::hover());
                        header_line(ui, palette, message, line, false);
                    }
                    bubble = bubble_frame(
                        ui,
                        app,
                        conversation,
                        message,
                        (fill, stroke),
                        text_width,
                        actions,
                    );
                    if !message.reactions.is_empty() {
                        // The pills tuck 1 point into the bubble's bottom edge.
                        ui.add_space(-(ui.spacing().item_spacing.y + REACTION_OVERLAP));
                        reactions(ui, app, conversation, message, me, actions);
                    }
                    let pills = super::agents::pending_reactions(app, conversation, &message.id);
                    if !pills.is_empty() {
                        super::agents::reaction_pills(ui, app, &pills, actions);
                    }
                });
            });
        }
        ui.add_space(1.0);
    });
    let rect = egui::Rect::from_min_max(
        start,
        egui::pos2(start.x + width, response.response.rect.bottom()),
    );
    // A grouped message shows its time beside the bubble on hover.
    if grouped && ui.rect_contains_pointer(rect) {
        let (at, align) = if mine {
            (
                egui::pos2(bubble.left() - 8.0, bubble.center().y),
                egui::Align2::RIGHT_CENTER,
            )
        } else {
            (
                egui::pos2(rect.left() + 2.0, bubble.center().y),
                egui::Align2::LEFT_CENTER,
            )
        };
        ui.painter().text(
            at,
            align,
            crate::time::clock(message.created),
            theme::regular(11.0),
            palette.dim,
        );
    }
    if highlighted {
        ui.painter().set(
            behind,
            egui::Shape::rect_filled(
                rect.expand2(vec2(8.0, 0.0)),
                CornerRadius::same(6),
                palette.highlight,
            ),
        );
    }
    find::outline_current(ui, app, &message.id, bubble);
    (rect, bubble)
}

/// The author, time and importance over a group of messages. The person's
/// own messages show only the time, on the right.
fn header_line(
    ui: &egui::Ui,
    palette: &theme::Palette,
    message: &Message,
    line: egui::Rect,
    mine: bool,
) {
    let mut parts = Vec::new();
    if !mine {
        parts.push((
            message.author.clone(),
            theme::regular(12.0),
            palette.secondary,
        ));
    }
    parts.push((
        crate::time::clock(message.created),
        theme::regular(12.0),
        palette.dim,
    ));
    if message.importance_high {
        parts.push(("IMPORTANT".to_owned(), theme::bold(11.0), palette.danger));
    }
    let gap = 8.0;
    let galleys: Vec<_> = parts
        .into_iter()
        .map(|(text, font, color)| {
            (
                ui.ctx().fonts_mut(|f| f.layout_no_wrap(text, font, color)),
                color,
            )
        })
        .collect();
    let total = galleys.iter().map(|(g, _)| g.size().x).sum::<f32>()
        + gap * galleys.len().saturating_sub(1) as f32;
    let mut x = if mine {
        line.right() - total
    } else {
        line.left()
    };
    let time_index = usize::from(!mine);
    for (index, (galley, color)) in galleys.into_iter().enumerate() {
        let size = galley.size();
        let at = egui::pos2(x, line.bottom() - size.y - 2.0);
        if index == time_index {
            ui.interact(
                egui::Rect::from_min_size(at, size),
                egui::Id::new(("chaat-time", &message.id)),
                Sense::hover(),
            )
            .on_hover_text(crate::time::day_label(message.created, crate::time::now()));
        }
        ui.painter().galley(at, galley, color);
        x += size.x + gap;
    }
}

/// Lays `add` out against the right edge of `ui`. Its width is known only
/// afterwards, so it comes from the frame before; the first frame lays it
/// out invisibly and asks for one more.
fn right_aligned<R>(
    ui: &mut egui::Ui,
    id: egui::Id,
    max_width: f32,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> R {
    let known: Option<f32> = ui.ctx().data(|d| d.get_temp(id));
    let right = ui.max_rect().right();
    let left = right - known.unwrap_or(max_width).min(max_width);
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(egui::Rect::from_min_size(
                egui::pos2(left, ui.cursor().top()),
                vec2(max_width, 0.0),
            ))
            .layout(Layout::top_down(Align::Min)),
    );
    if known.is_none() {
        child.set_invisible();
    }
    let inner = add(&mut child);
    let used = child.min_rect();
    if known.is_none_or(|k| (k - used.width()).abs() > 0.5) {
        ui.ctx().data_mut(|d| d.insert_temp(id, used.width()));
        ui.ctx().request_repaint();
    }
    // Content wider than the row (for a frame, before it is measured) must
    // not widen the history: every later row would take that width.
    ui.advance_cursor_after_rect(egui::Rect::from_min_max(
        used.min,
        egui::pos2(used.max.x.min(right), used.max.y),
    ));
    inner
}

/// A message's bubble. Returns its rect.
#[allow(clippy::too_many_arguments)]
fn bubble_frame(
    ui: &mut egui::Ui,
    app: &App,
    conversation: &ConversationId,
    message: &Message,
    (fill, stroke): (egui::Color32, Stroke),
    text_width: f32,
    actions: &mut Vec<Action>,
) -> egui::Rect {
    Frame::new()
        .fill(fill)
        .stroke(stroke)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(BUBBLE_PAD)
        .show(ui, |ui| {
            body(ui, app, conversation, message, text_width, actions);
        })
        .response
        .rect
}

/// A message's content below its header: quote, text, attachments,
/// delivery state. Reactions go below it.
fn body(
    ui: &mut egui::Ui,
    app: &App,
    conversation: &ConversationId,
    message: &Message,
    width: f32,
    actions: &mut Vec<Action>,
) {
    let palette = &app.palette;
    if message.deleted {
        ui.label(
            egui::RichText::new("This message was deleted.")
                .italics()
                .color(palette.dim),
        );
        return;
    }
    if let Some(quote) = message.quote()
        && let AttachmentKind::Quote {
            author,
            preview,
            message_id,
        } = &quote.kind
    {
        let response = rich::quote_block(ui, palette, author.as_deref(), preview, width.min(560.0));
        let response = response
            .interact(Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        widgets::hover_wash(ui, palette, &response, 6);
        if response.clicked() {
            actions.push(Action::OpenMessage {
                conversation: conversation.clone(),
                message: message_id.clone(),
            });
        }
    }
    let look = rich::Look {
        size: 14.0,
        color: if message.delivery == Delivery::Sent {
            palette.text
        } else {
            palette.secondary
        },
        // Quotes, files and cards make it an ordinary message.
        jumbo: message.attachments.is_empty() && rich::body_is_emoji_only(&message.body),
    };
    rich::show(ui, app, &message.body, &look, width, actions);
    for attachment in &message.attachments {
        match &attachment.kind {
            AttachmentKind::File => file_chip(ui, app, attachment, actions),
            AttachmentKind::Card => {
                if let Some(card) = attachment.content.as_deref().and_then(crate::cards::parse) {
                    super::cards::show(ui, palette, &card, width, actions);
                    continue;
                }
                ui.label(
                    egui::RichText::new(
                        "A card Chaat can't show yet. Open the message in Teams to see it.",
                    )
                    .font(theme::regular(12.0))
                    .color(palette.dim),
                );
            }
            _ => {}
        }
    }
    if message.edited {
        ui.label(
            egui::RichText::new("Edited")
                .font(theme::regular(11.0))
                .color(palette.dim),
        );
    }
    match message.delivery {
        Delivery::Sending => {
            ui.label(
                egui::RichText::new("Sending…")
                    .font(theme::regular(11.0))
                    .color(palette.dim),
            );
        }
        Delivery::Failed => {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Not sent.")
                        .font(theme::regular(12.0))
                        .color(palette.danger),
                );
                if ui
                    .link(egui::RichText::new("Retry").font(theme::semibold(12.0)))
                    .clicked()
                {
                    actions.push(Action::RetrySend {
                        conversation: conversation.clone(),
                        local_id: message.id.clone(),
                    });
                }
                ui.label(
                    egui::RichText::new("·")
                        .font(theme::regular(12.0))
                        .color(palette.dim),
                );
                if ui
                    .link(egui::RichText::new("Delete").font(theme::semibold(12.0)))
                    .clicked()
                {
                    actions.push(Action::DeleteFailed {
                        conversation: conversation.clone(),
                        local_id: message.id.clone(),
                    });
                }
            });
        }
        Delivery::Sent => {}
    }
}

fn file_chip(
    ui: &mut egui::Ui,
    app: &App,
    attachment: &crate::model::Attachment,
    actions: &mut Vec<Action>,
) {
    let palette = &app.palette;
    let name = attachment
        .name
        .clone()
        .unwrap_or_else(|| "Attachment".into());
    let response = Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(1.0, palette.outline))
        .corner_radius(CornerRadius::same(8))
        .inner_margin(Margin::symmetric(10, 7))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::icon(ui, Icon::File, 18.0, palette.accent);
                // A long name ends in "…" inside the bubble, keeping room
                // for the link icon; it must not widen the history.
                ui.scope(|ui| {
                    ui.set_max_width((ui.available_width() - 20.0).max(40.0));
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(&name)
                                .font(theme::medium(13.0))
                                .color(palette.text),
                        )
                        .truncate(),
                    );
                });
                if attachment.url.is_some() {
                    theme::icon(ui, Icon::ExternalLink, 13.0, palette.dim);
                }
            });
        })
        .response
        .interact(Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Open in your browser");
    widgets::hover_wash(ui, palette, &response, 8);
    if response.clicked()
        && let Some(url) = &attachment.url
    {
        actions.push(Action::OpenUrl(url.clone()));
    }
}

/// How far reaction pills overlap the bottom edge of a chat bubble.
const REACTION_OVERLAP: f32 = 1.0;

/// Reactions under a message: the emoji, with a count when more than one
/// person reacted, the person's own outlined in the accent; then a button
/// to add one.
fn reactions(
    ui: &mut egui::Ui,
    app: &App,
    conversation: &ConversationId,
    message: &Message,
    me: &str,
    actions: &mut Vec<Action>,
) {
    let palette = &app.palette;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for reaction in &message.reactions {
            let mine = reaction.users.iter().any(|u| u == me);
            let mut job = egui::text::LayoutJob::default();
            job.append(
                &reaction.emoji,
                0.0,
                egui::TextFormat {
                    font_id: theme::regular(18.0),
                    color: palette.text,
                    valign: Align::Center,
                    ..Default::default()
                },
            );
            let count = reaction.users.len();
            if count > 1 {
                job.append(
                    &count.to_string(),
                    5.0,
                    egui::TextFormat {
                        font_id: theme::medium(12.0),
                        color: if mine {
                            palette.accent
                        } else {
                            palette.secondary
                        },
                        valign: Align::Center,
                        ..Default::default()
                    },
                );
            }
            let button = egui::Button::new(job)
                .stroke(Stroke::new(
                    1.0,
                    if mine {
                        palette.accent
                    } else {
                        palette.outline
                    },
                ))
                .corner_radius(CornerRadius::same(13))
                .min_size(vec2(32.0, 26.0));
            let names: Vec<String> = reaction
                .users
                .iter()
                .map(|u| {
                    if u == me {
                        "You".to_owned()
                    } else {
                        app.chats
                            .iter()
                            .flat_map(|c| c.members.iter())
                            .find(|m| &m.user_id == u)
                            .map(|m| m.display_name.clone())
                            .unwrap_or_else(|| "Someone".into())
                    }
                })
                .collect();
            let response = widgets::with_fills(ui, palette.bubble(), palette.surface_hover, |ui| {
                // Snug around the emoji: a little room at the sides, almost
                // none above and below.
                ui.spacing_mut().button_padding = vec2(6.0, 0.0);
                ui.add(button)
            })
            .on_hover_text(names.join(", "));
            if response.clicked() {
                actions.push(Action::React {
                    conversation: conversation.clone(),
                    message: message.id.clone(),
                    emoji: reaction.emoji.clone(),
                    on: !mine,
                });
            }
        }
        let add = widgets::with_fills(ui, palette.window, palette.surface_hover, |ui| {
            ui.add(
                egui::Button::image(Icon::Smile.image(palette.secondary, 16.0))
                    .stroke(Stroke::new(1.0, palette.outline))
                    .corner_radius(CornerRadius::same(13))
                    .min_size(vec2(34.0, 26.0)),
            )
        })
        .on_hover_text("Add a reaction");
        egui::Popup::menu(&add)
            .id(egui::Id::new(("chaat-add-reaction", &message.id)))
            .show(|ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for emoji in QUICK_REACTIONS {
                        let on = message
                            .reactions
                            .iter()
                            .any(|r| r.emoji == emoji && r.users.iter().any(|u| u == me));
                        let response = emoji_button(
                            ui,
                            palette,
                            emoji,
                            vec2(36.0, 34.0),
                            20.0,
                            palette.text,
                            on,
                        );
                        if response.clicked() {
                            actions.push(Action::React {
                                conversation: conversation.clone(),
                                message: message.id.clone(),
                                emoji: emoji.to_owned(),
                                on: !on,
                            });
                        }
                    }
                });
            });
    });
}

/// Where the hover toolbar goes: over a chat bubble's top edge, or at the
/// right end of a channel post.
struct Placement {
    row: egui::Rect,
    bubble: Option<egui::Rect>,
}

/// The actions shown over the hovered message: quick reactions, reply, and
/// a menu with the rest. Returns the toolbar's rect.
#[allow(clippy::too_many_arguments)]
fn hover_toolbar(
    ui: &mut egui::Ui,
    app: &App,
    conversation: &ConversationId,
    id: &str,
    message: Option<&Message>,
    placement: Placement,
    mine: bool,
    menu_id: egui::Id,
    actions: &mut Vec<Action>,
) -> egui::Rect {
    let palette = &app.palette;
    let can_edit =
        mine && (!conversation.is_channel() || app.has_scope("ChannelMessage.ReadWrite"));
    let size = vec2(
        QUICK_REACTIONS.len() as f32 * 32.0 + 28.0 + 9.0 + 2.0 * 28.0 + 6.0,
        36.0,
    );
    let clip = ui.clip_rect();
    let row = placement.row;
    let min = match placement.bubble {
        Some(bubble) => {
            // The bar lines up with the bubble edge nearest the middle:
            // the left of your own, the right of everyone else's.
            let x = if mine {
                bubble.left()
            } else {
                bubble.right() - size.x
            };
            egui::pos2(
                x.clamp(row.left(), (row.right() - size.x).max(row.left())),
                (bubble.top() - size.y + 1.0).max(clip.top() + 2.0),
            )
        }
        None => egui::pos2(
            row.right() - size.x - 4.0,
            (row.top() - 12.0).max(clip.top() + 2.0),
        ),
    };
    let rect = egui::Rect::from_min_size(min, size);
    // Pointing at one quick reaction dims the others. Worked out from the
    // pointer, since the buttons draw left to right before their hover is
    // known.
    let pointed = ui
        .input(|i| i.pointer.hover_pos())
        .filter(|p| rect.contains(*p))
        .map(|p| ((p.x - rect.left() - 3.0) / 32.0).floor())
        .filter(|i| (0.0..QUICK_REACTIONS.len() as f32).contains(i))
        .map(|i| i as usize);
    let reacted = |emoji: &str| {
        message.is_some_and(|m| {
            m.reactions
                .iter()
                .any(|r| r.emoji == emoji && r.users.iter().any(|u| u == app.me()))
        })
    };
    ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
        Frame::new()
            .fill(palette.surface)
            .stroke(Stroke::new(1.0, palette.outline))
            .corner_radius(CornerRadius::same(8))
            // A deeper shadow lifts the bar off the message it overlaps.
            .shadow(egui::Shadow {
                offset: [0, 4],
                blur: 14,
                spread: 1,
                color: palette.shadow,
            })
            .inner_margin(Margin::same(3))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for (index, emoji) in QUICK_REACTIONS.into_iter().enumerate() {
                        let on = reacted(emoji);
                        let color = if pointed.is_none_or(|p| p == index) {
                            palette.text
                        } else {
                            palette.text.gamma_multiply(0.35)
                        };
                        let response =
                            emoji_button(ui, palette, emoji, vec2(32.0, 30.0), 18.0, color, on)
                                .on_hover_text(QUICK_REACTION_NAMES[index]);
                        if response.clicked() {
                            actions.push(Action::React {
                                conversation: conversation.clone(),
                                message: id.to_owned(),
                                emoji: emoji.to_owned(),
                                on: !on,
                            });
                        }
                    }
                    let more_reactions = widgets::icon_button_sized(
                        ui,
                        palette,
                        Icon::Plus,
                        "More reactions",
                        28.0,
                        16.0,
                        palette.secondary,
                    );
                    if let Some(emoji) = composer_tools::emoji_popup(
                        palette,
                        &more_reactions,
                        menu_id.with("react-popup"),
                        menu_id.with("react-picker"),
                    ) {
                        let on = !reacted(&emoji);
                        actions.push(Action::React {
                            conversation: conversation.clone(),
                            message: id.to_owned(),
                            emoji,
                            on,
                        });
                    }
                    let (separator, _) = ui.allocate_exact_size(vec2(9.0, 30.0), Sense::hover());
                    ui.painter().vline(
                        separator.center().x,
                        separator.y_range().shrink(5.0),
                        Stroke::new(1.0, palette.outline),
                    );
                    let reply_label = if conversation.is_channel() {
                        "Reply in thread"
                    } else {
                        "Reply"
                    };
                    let reply = widgets::icon_button_sized(
                        ui,
                        palette,
                        Icon::Reply,
                        reply_label,
                        28.0,
                        16.0,
                        palette.secondary,
                    );
                    widgets::on_click(&reply, actions, Action::StartReply(id.to_owned()));
                    let more = widgets::icon_button_sized(
                        ui,
                        palette,
                        Icon::More,
                        "More actions",
                        28.0,
                        16.0,
                        palette.secondary,
                    );
                    egui::Popup::menu(&more).id(menu_id).show(|ui| {
                        ui.set_min_width(160.0);
                        if can_edit {
                            if widgets::menu_item(ui, palette, Icon::Pencil, "Edit") {
                                actions.push(Action::StartEdit(id.to_owned()));
                            }
                            if widgets::menu_item(ui, palette, Icon::Trash, "Delete") {
                                actions.push(Action::Delete {
                                    conversation: conversation.clone(),
                                    message: id.to_owned(),
                                });
                            }
                        }
                        if widgets::menu_item(ui, palette, Icon::Copy, "Copy text")
                            && let Some(message) = message
                        {
                            actions.push(Action::CopyText(message.body.to_plain()));
                        }
                        if let ConversationId::Chat(chat) = conversation
                            && message.is_some_and(|m| !m.system)
                        {
                            let pin = app
                                .pins
                                .get(chat)
                                .and_then(|pins| pins.iter().find(|p| p.message_id == id));
                            match pin {
                                Some(pin) => {
                                    if widgets::menu_item(ui, palette, PIN_ICON, "Unpin") {
                                        actions.push(Action::UnpinMessage {
                                            chat: chat.clone(),
                                            pin: pin.id.clone(),
                                        });
                                    }
                                }
                                None => {
                                    if widgets::menu_item(ui, palette, PIN_ICON, "Pin") {
                                        actions.push(Action::PinMessage {
                                            chat: chat.clone(),
                                            message: id.to_owned(),
                                        });
                                    }
                                }
                            }
                        }
                        if let Some(message) = message
                            && !message.system
                            && crate::app::message_link(
                                conversation,
                                id,
                                message.web_url.as_deref(),
                            )
                            .is_some()
                            && widgets::menu_item(ui, palette, Icon::Link, "Copy link")
                        {
                            actions.push(Action::CopyMessageLink {
                                conversation: conversation.clone(),
                                message: id.to_owned(),
                            });
                        }
                    });
                });
            });
    });
    rect
}

/// A quick-reaction emoji: highlighted when the person has reacted with it,
/// and lit up under the pointer.
fn emoji_button(
    ui: &mut egui::Ui,
    palette: &theme::Palette,
    emoji: &str,
    size: egui::Vec2,
    font_size: f32,
    color: egui::Color32,
    on: bool,
) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() || response.has_focus();
        let fill = match (on, hovered) {
            (true, _) => Some(palette.highlight),
            (false, true) => Some(palette.surface_hover),
            (false, false) => None,
        };
        if let Some(fill) = fill {
            ui.painter().rect_filled(rect, CornerRadius::same(6), fill);
        }
        if on && hovered {
            ui.painter().rect_stroke(
                rect,
                CornerRadius::same(6),
                Stroke::new(1.0, palette.outline),
                egui::StrokeKind::Inside,
            );
        }
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            emoji,
            egui::FontId::proportional(font_size),
            color,
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

#[allow(clippy::too_many_arguments)]
fn thread_card(
    ui: &mut egui::Ui,
    app: &App,
    conversation: &ConversationId,
    root: &Message,
    me: &str,
    now: i64,
    expanded: bool,
    actions: &mut Vec<Action>,
) -> egui::Rect {
    let palette = &app.palette;
    let width = ui.available_width();
    if root.system {
        let width = ui.available_width();
        return message_row(
            ui,
            app,
            conversation,
            root,
            me,
            false,
            false,
            width,
            actions,
        )
        .0;
    }
    ui.add_space(6.0);
    let response = Frame::new()
        .fill(palette.surface)
        .stroke(Stroke::new(
            1.0,
            if root.importance_high {
                palette.danger.gamma_multiply(0.6)
            } else {
                palette.outline
            },
        ))
        .corner_radius(CornerRadius::same(10))
        .inner_margin(Margin {
            left: 12,
            right: 14,
            top: 10,
            bottom: 8,
        })
        .show(ui, |ui| {
            ui.set_width(width - 26.0);
            let inner_width = width - 26.0;
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = 10.0;
                widgets::avatar(ui, app, root.author_id.as_deref(), &root.author, 34.0, true);
                ui.vertical(|ui| {
                    let content = inner_width - 44.0;
                    ui.set_max_width(content);
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.horizontal(|ui| {
                        ui.label(
                            egui::RichText::new(&root.author)
                                .font(theme::semibold(14.0))
                                .color(palette.text),
                        );
                        ui.label(
                            egui::RichText::new(stamp(root.created, now))
                                .font(theme::regular(12.0))
                                .color(palette.dim),
                        );
                        if root.importance_high {
                            ui.label(
                                egui::RichText::new("IMPORTANT")
                                    .font(theme::bold(11.0))
                                    .color(palette.danger),
                            );
                        }
                    });
                    if let Some(subject) = &root.subject {
                        ui.label(
                            egui::RichText::new(subject)
                                .font(theme::bold(15.0))
                                .color(palette.text),
                        );
                    }
                    body(ui, app, conversation, root, content, actions);
                    if !root.reactions.is_empty() {
                        reactions(ui, app, conversation, root, me, actions);
                    }
                });
            });
            // Replies.
            ui.add_space(6.0);
            let count = root.replies.len();
            if count > 0 {
                let line = ui.cursor().top();
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    line,
                    Stroke::new(1.0, palette.outline),
                );
                ui.add_space(6.0);
                if !expanded && count > 2 {
                    let mut names: Vec<&str> =
                        root.replies.iter().map(|r| r.author.as_str()).collect();
                    names.sort_unstable();
                    names.dedup();
                    let text = format!(
                        "{count} replies from {}",
                        names.into_iter().take(3).collect::<Vec<_>>().join(", ")
                    );
                    if ui
                        .link(
                            egui::RichText::new(text)
                                .font(theme::semibold(12.0))
                                .color(palette.accent),
                        )
                        .clicked()
                    {
                        actions.push(Action::ToggleThread(root.id.clone()));
                    }
                }
                let shown: Vec<&Message> = if expanded || count <= 2 {
                    root.replies.iter().collect()
                } else {
                    root.replies.iter().rev().take(1).collect()
                };
                for reply in shown {
                    ui.horizontal_top(|ui| {
                        ui.add_space(8.0);
                        ui.spacing_mut().item_spacing.x = 8.0;
                        widgets::avatar(
                            ui,
                            app,
                            reply.author_id.as_deref(),
                            &reply.author,
                            26.0,
                            true,
                        );
                        ui.vertical(|ui| {
                            let content = inner_width - 52.0;
                            ui.set_max_width(content);
                            ui.spacing_mut().item_spacing.y = 2.0;
                            ui.horizontal(|ui| {
                                ui.label(
                                    egui::RichText::new(&reply.author)
                                        .font(theme::semibold(13.0))
                                        .color(palette.text),
                                );
                                ui.label(
                                    egui::RichText::new(crate::time::clock(reply.created))
                                        .font(theme::regular(11.0))
                                        .color(palette.dim),
                                );
                            });
                            body(ui, app, conversation, reply, content, actions);
                            if !reply.reactions.is_empty() {
                                reactions(ui, app, conversation, reply, me, actions);
                            }
                        });
                    });
                    ui.add_space(4.0);
                }
                if expanded
                    && count > 2
                    && ui
                        .link(
                            egui::RichText::new("Collapse")
                                .font(theme::regular(12.0))
                                .color(palette.secondary),
                        )
                        .clicked()
                {
                    actions.push(Action::ToggleThread(root.id.clone()));
                }
            }
            ui.horizontal(|ui| {
                let replying = app
                    .drafts
                    .get(conversation)
                    .and_then(|d| d.reply_to.as_deref())
                    == Some(root.id.as_str());
                let label = if replying {
                    "Replying below…"
                } else {
                    "Reply"
                };
                let response = widgets::text_button(ui, palette, label, theme::medium(12.0));
                if response.clicked() {
                    actions.push(Action::StartReply(root.id.clone()));
                }
            });
        });
    ui.add_space(6.0);
    response.response.rect
}

// -- Composer -----------------------------------------------------------------

/// The `@name` being typed before the cursor: where its `@` is (in
/// characters) and what follows it. Names have spaces, so up to two words
/// count; a line break or a third word ends it.
pub fn mention_query(text: &str, cursor: usize) -> Option<(usize, String)> {
    let chars: Vec<char> = text.chars().collect();
    let cursor = cursor.min(chars.len());
    let mut at = None;
    for index in (0..cursor).rev() {
        let c = chars[index];
        if c == '\n' || cursor - index > 32 {
            return None;
        }
        if c == '@' {
            at = Some(index);
            break;
        }
    }
    let at = at?;
    if at > 0 && chars[at - 1].is_alphanumeric() {
        // An email address, not a mention.
        return None;
    }
    let query: String = chars[at + 1..cursor].iter().collect();
    if query.split(' ').count() > 3 || query.starts_with(' ') {
        return None;
    }
    Some((at, query))
}

/// People who can be mentioned here whose name matches `query`.
fn mention_candidates(
    app: &App,
    conversation: &ConversationId,
    query: &str,
) -> Vec<crate::model::Member> {
    let needle = query.trim().to_lowercase();
    let me = app.me().to_owned();
    let matches = |name: &str| {
        needle.is_empty()
            || name
                .to_lowercase()
                .split_whitespace()
                .any(|word| word.starts_with(&needle))
            || name.to_lowercase().starts_with(&needle)
    };
    let mut people: Vec<crate::model::Member> = match conversation {
        ConversationId::Chat(chat) => app
            .chat(chat)
            .map(|c| {
                c.members
                    .iter()
                    .filter(|m| m.user_id != me)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default(),
        ConversationId::Channel { .. } => app.directory(),
    };
    if conversation.is_channel() && app.people.query == query {
        for person in &app.people.results {
            if !people.iter().any(|p| p.user_id == person.user_id) {
                people.push(person.clone());
            }
        }
    }
    people.retain(|p| matches(&p.display_name));
    people.truncate(6);
    people
}

fn file_size(path: &std::path::Path) -> String {
    let bytes = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    match bytes {
        b if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        b if b >= 1024 => format!("{} KB", b / 1024),
        b => format!("{b} B"),
    }
}

fn composer(app: &mut App, ui: &mut egui::Ui, conversation: &ConversationId) {
    let palette = app.palette;
    let title = app.title_of(conversation);
    let channel = conversation.is_channel();
    let mut actions = Vec::new();
    let mut draft = app.drafts.get(conversation).cloned().unwrap_or_default();
    let context_line = draft_context(app, conversation, &draft);
    let can_post = !channel || app.has_scope("ChannelMessage.Send");
    let id = ui.make_persistent_id(("composer", conversation.key()));
    let pasted_cursor = draft.paste_cursor.take();
    if let Some(cursor) = pasted_cursor {
        let mut state = egui::TextEdit::load_state(ui.ctx(), id).unwrap_or_default();
        state
            .cursor
            .set_char_range(Some(egui::text::CCursorRange::one(
                egui::text::CCursor::new(cursor),
            )));
        state.store(ui.ctx(), id);
    }
    // An emoji picked last frame goes in at the caret.
    let emoji_pick_id = id.with("emoji-pick");
    if let Some(emoji) = ui
        .ctx()
        .data_mut(|d| d.remove_temp::<String>(emoji_pick_id))
    {
        composer_tools::insert_at_caret(ui.ctx(), id, &mut draft.text, &emoji);
    }
    let format_on = composer_tools::format_on(ui.ctx());
    // The mention being typed, from the cursor the editor had last frame.
    let cursor = egui::TextEdit::load_state(ui.ctx(), id)
        .and_then(|state| state.cursor.char_range())
        .map(|range| range.primary.index.0);
    let dismissed_id = id.with("mention-dismissed");
    let dismissed: Option<usize> = ui.ctx().data(|d| d.get_temp(dismissed_id));
    let typed = cursor.and_then(|cursor| {
        mention_query(&draft.text, cursor).map(|(at, query)| (at, query, cursor))
    });
    // Esc dismisses one `@`. Once it is gone, a new one in the same place
    // shows the list again.
    if dismissed.is_some() && typed.as_ref().map(|(at, ..)| *at) != dismissed {
        ui.ctx().data_mut(|d| d.remove_temp::<usize>(dismissed_id));
    }
    let mention = typed.filter(|(at, ..)| dismissed != Some(*at));
    let candidates = mention
        .as_ref()
        .map(|(_, query, _)| mention_candidates(app, conversation, query))
        .unwrap_or_default();
    if let Some((_, query, _)) = &mention
        && channel
        && query.chars().count() >= 2
        && app.people.query != *query
    {
        actions.push(Action::SearchPeople(query.clone()));
    }
    let selected_id = id.with("mention-selected");
    let mut selected: usize = ui.ctx().data(|d| d.get_temp(selected_id)).unwrap_or(0);
    selected = selected.min(candidates.len().saturating_sub(1));
    // A person clicked in the list last frame.
    let pick_id = id.with("mention-pick");
    let clicked: Option<usize> = ui.ctx().data_mut(|d| d.remove_temp(pick_id));
    let mut picked: Option<crate::model::Member> =
        clicked.and_then(|index| candidates.get(index).cloned());
    let mut composer_top = None;
    let max_editor_height = (ui.available_height() / 3.0).clamp(40.0, 200.0);
    egui::Panel::bottom("composer")
        .show_separator_line(false)
        .frame(Frame::new().fill(palette.window).inner_margin(Margin {
            left: 16,
            right: 16,
            top: 6,
            bottom: 14,
        }))
        .show(ui, |ui| {
            super::agents::proposal_card(ui, app, conversation, &mut actions);
            composer_top = Some(ui.cursor().min);
            Frame::new()
                .fill(palette.surface)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(10))
                .inner_margin(Margin {
                    left: 12,
                    right: 8,
                    top: 6,
                    bottom: 6,
                })
                .show(ui, |ui| {
                    if let Some((icon, text)) = &context_line {
                        ui.horizontal(|ui| {
                            theme::icon(ui, *icon, 14.0, palette.accent);
                            let width = ui.available_width() - 40.0;
                            ui.add(egui::Label::new(super::widgets::elided(
                                ui,
                                text,
                                theme::regular(12.0),
                                palette.secondary,
                                width,
                            )));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let close = widgets::icon_button_sized(
                                    ui,
                                    &palette,
                                    Icon::Close,
                                    "Cancel (Esc)",
                                    22.0,
                                    14.0,
                                    palette.dim,
                                );
                                widgets::on_click(&close, &mut actions, Action::CancelCompose);
                            });
                        });
                        ui.add_space(2.0);
                    }
                    // Files waiting to go with the message.
                    if !draft.files.is_empty() {
                        let mut remove = None;
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                            for (index, path) in draft.files.iter().enumerate() {
                                let name = path
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                Frame::new()
                                    .fill(palette.surface_hover)
                                    .corner_radius(CornerRadius::same(6))
                                    .inner_margin(Margin {
                                        left: 8,
                                        right: 2,
                                        top: 3,
                                        bottom: 3,
                                    })
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.spacing_mut().item_spacing.x = 4.0;
                                            theme::icon(ui, Icon::File, 14.0, palette.accent);
                                            ui.label(
                                                egui::RichText::new(crate::html::truncate(
                                                    &name, 32,
                                                ))
                                                .font(theme::medium(12.0))
                                                .color(palette.text),
                                            );
                                            ui.label(
                                                egui::RichText::new(file_size(path))
                                                    .font(theme::regular(11.0))
                                                    .color(palette.dim),
                                            );
                                            let close = widgets::icon_button_sized(
                                                ui,
                                                &palette,
                                                Icon::Close,
                                                "Remove",
                                                20.0,
                                                12.0,
                                                palette.dim,
                                            );
                                            if close.clicked() {
                                                remove = Some(index);
                                            }
                                        });
                                    });
                            }
                        });
                        if let Some(index) = remove {
                            draft.files.remove(index);
                        }
                        ui.add_space(4.0);
                    }
                    if !draft.images.is_empty() {
                        let mut remove = None;
                        ui.horizontal_wrapped(|ui| {
                            for (index, image) in draft.images.iter().enumerate() {
                                let group_size =
                                    vec2(144.0 + ui.spacing().item_spacing.x + 22.0, 96.0);
                                ui.allocate_ui_with_layout(
                                    group_size,
                                    Layout::left_to_right(Align::Center),
                                    |ui| {
                                        // Crop a fixed-size thumbnail instead of shrinking a
                                        // wide screenshot into a line. The preview keeps
                                        // the full image and its original proportions.
                                        let size = vec2(144.0, 96.0);
                                        let natural = vec2(image.width as f32, image.height as f32);
                                        let scale = (size.x / natural.x).max(size.y / natural.y);
                                        let uv = egui::Rect::from_center_size(
                                            egui::pos2(0.5, 0.5),
                                            size / (natural * scale),
                                        );
                                        let response = ui
                                            .add(
                                                egui::Image::new(crate::images::cropped(
                                                    &image.uri(),
                                                ))
                                                .fit_to_exact_size(size)
                                                .maintain_aspect_ratio(false)
                                                .uv(uv)
                                                .corner_radius(6)
                                                .sense(Sense::click())
                                                .alt_text("Preview pasted image"),
                                            )
                                            .on_hover_cursor(egui::CursorIcon::PointingHand)
                                            .on_hover_text("Preview image");
                                        widgets::hover_wash(ui, &palette, &response, 6);
                                        if response.clicked() {
                                            actions.push(Action::PreviewImage(image.uri()));
                                        }
                                        if widgets::icon_button_sized(
                                            ui,
                                            &palette,
                                            Icon::Close,
                                            "Remove image",
                                            22.0,
                                            14.0,
                                            palette.dim,
                                        )
                                        .clicked()
                                        {
                                            remove = Some(index);
                                        }
                                    },
                                );
                            }
                        });
                        if let Some(index) = remove {
                            draft.images.remove(index);
                        }
                        ui.add_space(4.0);
                    }
                    let bar_format = (format_on && can_post)
                        .then(|| composer_tools::format_row(ui, &palette))
                        .flatten();
                    if format_on {
                        // The editor sits right under the toolbar.
                        ui.spacing_mut().item_spacing.y = 0.0;
                    }
                    ui.horizontal(|ui| {
                        let focused = ui.memory(|m| m.has_focus(id));
                        let mut typed = false;
                        let mut shortcut = None;
                        let mut send = false;
                        let mut paste_requested = false;
                        let mut dismiss: Option<usize> = None;
                        let range = egui::TextEdit::load_state(ui.ctx(), id)
                            .and_then(|state| state.cursor.char_range())
                            .map(|r| {
                                (
                                    r.primary.index.0.min(r.secondary.index.0),
                                    r.primary.index.0.max(r.secondary.index.0),
                                )
                            })
                            .unwrap_or((draft.text.chars().count(), draft.text.chars().count()));
                        // Enter or Shift+Enter in a list item starts the next
                        // item; on an empty item it ends the list, and the
                        // next Enter sends. The mention list keeps Enter.
                        let in_list = range.0 == range.1
                            && candidates.is_empty()
                            && crate::compose_format::continue_list(&draft.text, range.0).is_some();
                        let mut continue_list = false;
                        if focused {
                            ui.input_mut(|input| {
                                shortcut = composer_tools::take_shortcut(&mut input.events);
                                typed = input
                                    .events
                                    .iter()
                                    .any(|e| matches!(e, egui::Event::Text(_)));
                                let mut pastes = Vec::new();
                                let mut enter = false;
                                input.events.retain(|event| {
                                    if let egui::Event::Paste(text) = event {
                                        pastes.push(text.clone());
                                        false
                                    } else if in_list
                                        && matches!(event, egui::Event::Key {
                                            key: Key::Enter, pressed: true, modifiers, ..
                                        } if modifiers.matches_exact(Modifiers::NONE)
                                            || modifiers.matches_exact(Modifiers::SHIFT))
                                    {
                                        continue_list = true;
                                        false
                                    } else if matches!(event, egui::Event::Key {
                                        key: Key::Enter, pressed: true, modifiers, ..
                                    } if modifiers.matches_exact(Modifiers::NONE))
                                    {
                                        // consume_key ignores extra Shift and Alt.
                                        // Keep Shift+Enter for the multiline editor.
                                        enter = true;
                                        false
                                    } else {
                                        true
                                    }
                                });
                                for text in pastes {
                                    paste_requested = true;
                                    actions.push(Action::Paste {
                                        conversation: conversation.clone(),
                                        range,
                                        text,
                                    });
                                }
                                // The mention list takes the arrows, Enter
                                // and Tab while it shows.
                                if !candidates.is_empty() {
                                    if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
                                        selected = (selected + 1) % candidates.len();
                                    }
                                    if input.consume_key(Modifiers::NONE, Key::ArrowUp) {
                                        selected =
                                            (selected + candidates.len() - 1) % candidates.len();
                                    }
                                    if enter || input.consume_key(Modifiers::NONE, Key::Tab) {
                                        picked = candidates.get(selected).cloned();
                                        enter = false;
                                    }
                                    if input.consume_key(Modifiers::NONE, Key::Escape)
                                        && let Some((at, ..)) = &mention
                                    {
                                        dismiss = Some(*at);
                                    }
                                }
                                send |= enter;
                                if input.consume_key(Modifiers::NONE, Key::Escape) {
                                    actions.push(Action::CancelCompose);
                                }
                                if draft.text.is_empty()
                                    && input.consume_key(Modifiers::NONE, Key::ArrowUp)
                                {
                                    let me = app.me();
                                    if let Some(last) =
                                        app.conversation(conversation).and_then(|c| {
                                            c.messages
                                                .iter()
                                                .rev()
                                                .find(|m| m.is_from(me) && !m.deleted && !m.system)
                                        })
                                        && !channel
                                    {
                                        actions.push(Action::StartEdit(last.id.clone()));
                                    }
                                }
                            });
                        } else if ui.memory(|m| m.had_focus_last_frame(id)) {
                            // Esc surrenders egui's focus before the frame.
                            ui.input_mut(|input| {
                                if input.consume_key(Modifiers::NONE, Key::Escape) {
                                    match &mention {
                                        Some((at, ..)) if !candidates.is_empty() => {
                                            dismiss = Some(*at);
                                        }
                                        _ => actions.push(Action::CancelCompose),
                                    }
                                }
                            });
                        }
                        // Written after input_mut: egui's context lock is not
                        // re-entrant, so writing memory inside it deadlocks.
                        if let Some(at) = dismiss {
                            ui.ctx().data_mut(|d| d.insert_temp(dismissed_id, at));
                        }
                        if continue_list {
                            composer_tools::continue_list_at_caret(ui.ctx(), id, &mut draft.text);
                        }
                        if let Some(format) = bar_format.or(shortcut) {
                            composer_tools::apply_format(ui.ctx(), id, &mut draft.text, format);
                        }
                        // Put the picked name in place of `@query`.
                        if let (Some(person), Some((at, _, cursor))) = (&picked, &mention) {
                            // Consuming Tab doesn't cancel egui's pending focus
                            // move. Keep keyboard input in the composer.
                            ui.memory_mut(|memory| memory.move_focus(egui::FocusDirection::None));
                            let chars: Vec<char> = draft.text.chars().collect();
                            let before: String = chars[..*at].iter().collect();
                            let after: String =
                                chars[(*cursor).min(chars.len())..].iter().collect();
                            let inserted = format!("@{} ", person.display_name);
                            draft.text = format!("{before}{inserted}{after}");
                            if !draft.mentions.iter().any(|m| m.user_id == person.user_id) {
                                draft.mentions.push(crate::model::MentionRef {
                                    user_id: person.user_id.clone(),
                                    name: person.display_name.clone(),
                                });
                            }
                            let caret = at + inserted.chars().count();
                            if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
                                state
                                    .cursor
                                    .set_char_range(Some(egui::text::CCursorRange::one(
                                        egui::text::CCursor::new(caret),
                                    )));
                                egui::TextEdit::store_state(ui.ctx(), id, state);
                            }
                        }
                        let hint = if !can_post {
                            "Posting needs the ChannelMessage.Send permission".to_owned()
                        } else if draft.editing.is_some() {
                            "Edit your message".to_owned()
                        } else if channel && draft.reply_to.is_none() {
                            format!("Start a new post in {title}  ·  @ to mention")
                        } else if channel {
                            "Reply to the thread".to_owned()
                        } else {
                            format!("Message {title}  ·  @ to mention")
                        };
                        let width = (ui.available_width() - 138.0).max(0.0);
                        let hint = composer_tools::fit_hint(ui, &hint, width);
                        let mut layouter =
                            |ui: &egui::Ui, text: &dyn egui::TextBuffer, wrap: f32| {
                                composer_tools::styled_galley(
                                    ui,
                                    &palette,
                                    id.with("galley"),
                                    text.as_str(),
                                    wrap,
                                )
                            };
                        let edit = egui::TextEdit::multiline(&mut draft.text)
                            .layouter(&mut layouter)
                            .id(id)
                            .hint_text(hint)
                            .desired_rows(1)
                            .desired_width(width)
                            // A custom Frame overrides TextEdit::margin.
                            // Put the padding on the frame itself.
                            .frame(Frame::new().inner_margin(Margin {
                                left: 0,
                                right: 0,
                                top: if format_on { 0 } else { 6 },
                                bottom: 6,
                            }))
                            .font(theme::regular(14.0))
                            .vertical_align(Align::Center);
                        // Reserve the controls' width and let only the editor
                        // scroll. TextEdit's desired_rows is a minimum, so a
                        // large paste otherwise grows over the message history.
                        let response = ui
                            .allocate_ui_with_layout(
                                vec2(width, max_editor_height),
                                Layout::top_down(Align::Min),
                                |ui| {
                                    egui::ScrollArea::vertical()
                                        .id_salt(id.with("scroll"))
                                        .max_height(max_editor_height)
                                        .min_scrolled_height(0.0)
                                        .auto_shrink([false, true])
                                        .animated(false)
                                        .show(ui, |ui| {
                                            ui.add_enabled_ui(can_post, |ui| {
                                                let output = edit.show(ui);
                                                // Paste and mention completion move the
                                                // cursor before TextEdit handles input.
                                                if (pasted_cursor.is_some() || picked.is_some())
                                                    && let Some(range) =
                                                        output.state.cursor.char_range()
                                                {
                                                    let caret = output
                                                        .galley
                                                        .pos_from_cursor(range.primary)
                                                        .translate(output.galley_pos.to_vec2());
                                                    ui.scroll_to_rect(caret, None);
                                                }
                                                output.response
                                            })
                                            .inner
                                        })
                                        .inner
                                },
                            )
                            .inner;
                        if std::mem::take(&mut app.focus_composer) {
                            response.request_focus();
                        }
                        // `:shortcode:` and `:) ` typed just now become emoji, unless
                        // a mention list is open.
                        let allowed =
                            typed && response.changed() && picked.is_none() && mention.is_none();
                        composer_tools::convert_typed_at_caret(
                            ui.ctx(),
                            id,
                            &mut draft.text,
                            allowed,
                        );
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.spacing_mut().item_spacing.x = 2.0;
                            let ready = !draft.text.trim().is_empty()
                                || !draft.files.is_empty()
                                || !draft.images.is_empty();
                            let tint = if ready { palette.accent } else { palette.dim };
                            let label = if draft.editing.is_some() {
                                "Save (Enter)"
                            } else {
                                "Send (Enter)"
                            };
                            let button = widgets::icon_button_sized(
                                ui,
                                &palette,
                                if draft.editing.is_some() {
                                    Icon::Check
                                } else {
                                    Icon::Send
                                },
                                label,
                                30.0,
                                18.0,
                                tint,
                            );
                            if button.clicked() {
                                send = true;
                            }
                            if draft.editing.is_none() {
                                let attach = widgets::icon_button_sized(
                                    ui,
                                    &palette,
                                    Icon::Paperclip,
                                    "Attach files (or drop them here)",
                                    30.0,
                                    18.0,
                                    palette.secondary,
                                );
                                widgets::on_click(&attach, &mut actions, Action::AttachFiles);
                            }
                            let smile = widgets::icon_button_sized(
                                ui,
                                &palette,
                                Icon::Smile,
                                "Emoji",
                                30.0,
                                18.0,
                                palette.secondary,
                            );
                            if let Some(emoji) = composer_tools::emoji_popup(
                                &palette,
                                &smile,
                                id.with("emoji-popup"),
                                id.with("emoji-picker"),
                            ) {
                                ui.ctx().data_mut(|d| d.insert_temp(emoji_pick_id, emoji));
                                ui.ctx().request_repaint();
                            }
                            if composer_tools::format_toggle(ui, &palette, format_on).clicked() {
                                composer_tools::set_format_on(ui.ctx(), !format_on);
                            }
                        });
                        let has_content = !draft.text.trim().is_empty()
                            || !draft.files.is_empty()
                            || !draft.images.is_empty();
                        if send
                            && has_content
                            && can_post
                            && picked.is_none()
                            && draft.pending_pastes == 0
                            && !paste_requested
                        {
                            if let Some(text) = composer_tools::convert_at_end(&draft.text) {
                                draft.text = text;
                            }
                            match &draft.editing {
                                Some(message) => actions.push(Action::Edit {
                                    conversation: conversation.clone(),
                                    message: message.clone(),
                                    text: std::mem::take(&mut draft.text),
                                }),
                                None => actions.push(Action::Send {
                                    conversation: conversation.clone(),
                                    text: std::mem::take(&mut draft.text),
                                    reply_to: draft.reply_to.take(),
                                    mentions: std::mem::take(&mut draft.mentions),
                                    files: std::mem::take(&mut draft.files),
                                    images: std::mem::take(&mut draft.images),
                                }),
                            }
                            draft.editing = None;
                        }
                    });
                });
            ui.add_space(2.0);
            // Formatting help only while typing. The line keeps its room
            // either way, so the history doesn't move as focus changes.
            let typing = ui.memory(|m| m.has_focus(id));
            ui.add(
                egui::Label::new(
                    egui::RichText::new(composer_tools::help_line(ui, format_on))
                        .font(theme::regular(11.0))
                        .color(if typing {
                            palette.dim
                        } else {
                            egui::Color32::TRANSPARENT
                        }),
                )
                .truncate(),
            );
        });
    ui.ctx().data_mut(|d| d.insert_temp(selected_id, selected));
    // The mention list, just above the composer.
    if picked.is_none()
        && !candidates.is_empty()
        && let Some(top) = composer_top
    {
        let height = candidates.len() as f32 * 40.0 + 12.0;
        egui::Area::new(id.with("mentions"))
            .order(egui::Order::Foreground)
            .fixed_pos(top + vec2(0.0, -height - 4.0))
            .show(ui.ctx(), |ui| {
                Frame::new()
                    .fill(palette.surface)
                    .stroke(Stroke::new(1.0, palette.outline))
                    .corner_radius(CornerRadius::same(10))
                    .shadow(egui::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: palette.shadow,
                    })
                    .inner_margin(Margin::same(6))
                    .show(ui, |ui| {
                        ui.set_width(300.0);
                        for (index, person) in candidates.iter().enumerate() {
                            let (rect, response) =
                                ui.allocate_exact_size(vec2(300.0, 38.0), Sense::click());
                            let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
                            if index == selected || response.hovered() {
                                ui.painter().rect_filled(
                                    rect,
                                    CornerRadius::same(7),
                                    palette.surface_active,
                                );
                            }
                            let mut child = ui.new_child(
                                egui::UiBuilder::new()
                                    .max_rect(rect.shrink2(vec2(8.0, 0.0)))
                                    .layout(Layout::left_to_right(Align::Center)),
                            );
                            widgets::avatar(
                                &mut child,
                                app,
                                Some(&person.user_id),
                                &person.display_name,
                                26.0,
                                true,
                            );
                            child.add_space(8.0);
                            child.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 0.0;
                                ui.label(
                                    egui::RichText::new(&person.display_name)
                                        .font(theme::medium(13.0))
                                        .color(palette.text),
                                );
                                if let Some(email) = &person.email {
                                    ui.label(
                                        egui::RichText::new(email)
                                            .font(theme::regular(11.0))
                                            .color(palette.dim),
                                    );
                                }
                            });
                            if response.clicked() {
                                // Put in place next frame, by the composer.
                                ui.ctx().data_mut(|d| d.insert_temp(pick_id, index));
                                ui.ctx().memory_mut(|m| m.request_focus(id));
                                ui.ctx().request_repaint();
                            }
                        }
                    });
            });
    }
    let changed = app.drafts.get(conversation).cloned().unwrap_or_default() != draft;
    let sending = actions
        .iter()
        .any(|a| matches!(a, Action::Send { .. } | Action::Edit { .. }));
    if changed && !sending {
        if draft == Draft::default() {
            app.drafts.remove(conversation);
        } else {
            app.drafts.insert(conversation.clone(), draft);
        }
    } else if sending {
        app.drafts.remove(conversation);
    }
    app.actions.extend(actions);
}

fn draft_context(
    app: &App,
    conversation: &ConversationId,
    draft: &Draft,
) -> Option<(Icon, String)> {
    if let Some(editing) = &draft.editing {
        let _ = editing;
        return Some((Icon::Pencil, "Editing your message".into()));
    }
    let reply_to = draft.reply_to.as_ref()?;
    let message = app.conversation(conversation)?.find(reply_to)?;
    let text = if conversation.is_channel() {
        let subject = message
            .subject
            .clone()
            .unwrap_or_else(|| message.body.summary(80));
        format!("Replying to {}: {subject}", message.author)
    } else {
        format!("Quoting {}: {}", message.author, message.body.summary(100))
    };
    Some((Icon::Reply, text))
}

fn channel_consent(app: &mut App, ui: &mut egui::Ui, conversation: &ConversationId) {
    let palette = app.palette;
    ui.vertical_centered(|ui| {
        ui.add_space((ui.available_height() * 0.22).max(24.0));
        ui.set_max_width(ui.available_width().min(460.0));
        theme::icon(ui, Icon::Lock, 36.0, palette.dim);
        ui.add_space(8.0);
        ui.label(egui::RichText::new("Channel posts need admin approval").font(theme::semibold(18.0)).color(palette.text));
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(
                "Reading channel posts uses Microsoft Graph's ChannelMessage.Read.All permission, which a tenant administrator must approve. You can still post here; posting needs only ChannelMessage.Send.",
            )
            .color(palette.secondary),
        );
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let total = 300.0;
            ui.add_space(((ui.available_width() - total) / 2.0).max(0.0));
            if widgets::primary_button(ui, &palette, "Request access").clicked() {
                app.actions.push(Action::RequestScope(crate::auth::CHANNEL_READ_SCOPE));
            }
            if let ConversationId::Channel { team, channel } = conversation
                && let Some(url) = app.channel(team, channel).and_then(|c| c.web_url.clone())
                && widgets::secondary_button(ui, &palette, "Open in Teams").clicked()
            {
                app.actions.push(Action::OpenUrl(url));
            }
        });
    });
}

/// `14:05` today, `Yesterday 14:05`, `Mon 14:05`, `2 Oct 14:05`.
fn stamp(at: i64, now: i64) -> String {
    if crate::time::day_key(at) == crate::time::day_key(now) {
        crate::time::clock(at)
    } else {
        format!(
            "{} {}",
            crate::time::short_label(at, now),
            crate::time::clock(at)
        )
    }
}

#[cfg(test)]
mod pin_tests {
    use super::step_pin;

    #[test]
    fn pin_index_wraps_both_ways() {
        assert_eq!(step_pin(0, 3, 1), 1);
        assert_eq!(step_pin(2, 3, 1), 0);
        assert_eq!(step_pin(0, 3, -1), 2);
        assert_eq!(step_pin(5, 3, 1), 0);
        assert_eq!(step_pin(0, 0, 1), 0);
    }
}

#[cfg(test)]
mod mention_tests {
    use super::mention_query;

    #[test]
    fn mentions_start_at_an_at_sign_and_end_at_a_line_break() {
        assert_eq!(mention_query("hi @ad", 6), Some((3, "ad".into())));
        assert_eq!(mention_query("@", 1), Some((0, String::new())));
        assert_eq!(mention_query("hi @Ada Lo", 10), Some((3, "Ada Lo".into())));
        assert_eq!(mention_query("mail me@contoso", 15), None);
        assert_eq!(mention_query("@ada\nnext", 9), None);
        assert_eq!(mention_query("no mention", 10), None);
        assert_eq!(mention_query("@a b c d e", 10), None);
    }
}
