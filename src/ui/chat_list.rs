//! The chat list: a filter field and a virtualized list (only the rows on
//! screen are laid out, however many chats there are).

use egui::{CornerRadius, Margin, Sense, vec2};

use super::widgets;
use crate::app::App;
use crate::model::{Action, Chat, ChatKind, ConversationId};
use crate::theme::{self, Icon};

/// Whether a chat matches the filter: its title or any member's name.
pub fn matches_filter(chat: &Chat, filter: &str, me: &str) -> bool {
    let filter = filter.trim().to_lowercase();
    if filter.is_empty() {
        return true;
    }
    chat.title(me).to_lowercase().contains(&filter)
        || chat
            .members
            .iter()
            .any(|m| m.display_name.to_lowercase().contains(&filter))
}

/// Whether a chat's kind passes the Groups and Meetings chips. With neither
/// on, every kind passes; with one or both on, only the chosen kinds do.
pub fn matches_kind(chat: &Chat, groups: bool, meetings: bool) -> bool {
    match chat.kind {
        _ if !groups && !meetings => true,
        ChatKind::Group => groups,
        ChatKind::Meeting => meetings,
        ChatKind::OneOnOne => false,
    }
}

/// Whether a chat shows in the list. With Unread on, a chat opened while
/// unread stays too, so it does not vanish the moment it is read. With Hidden
/// on, only hidden chats show (and Unread does not apply); otherwise none do.
pub fn listed(app: &App, chat: &Chat, me: &str) -> bool {
    let kept = app.unread_kept.as_deref() == Some(chat.id.as_str())
        && app.open.as_ref() == Some(&ConversationId::Chat(chat.id.clone()));
    chat.hidden == app.hidden_only
        && matches_filter(chat, &app.chat_filter, me)
        && matches_kind(chat, app.groups_only, app.meetings_only)
        && (app.hidden_only || !app.unread_only || chat.is_unread(me) || kept)
}

/// What the empty list says when the kind chips leave nothing to show.
fn kind_label(groups: bool, meetings: bool) -> &'static str {
    match (groups, meetings) {
        (true, true) => "group or meeting chats",
        (true, false) => "group chats",
        _ => "meeting chats",
    }
}

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    // Names Graph left out of system events in previews, asked for once.
    let unknown: Vec<String> = app
        .chats
        .iter()
        .filter_map(|chat| chat.preview.as_ref()?.membership.as_ref())
        .flat_map(|m| m.unnamed())
        .filter(|id| app.needs_name(id))
        .map(str::to_owned)
        .collect();
    if !unknown.is_empty() {
        app.actions.push(Action::LookUpNames(unknown));
    }
    egui::Frame::new()
        .inner_margin(Margin::symmetric(12, 0))
        .show(ui, |ui| {
            let field =
                widgets::text_field(&mut app.chat_filter, "Filter chats", theme::regular(13.0));
            let response = ui.add(field);
            if response.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                app.chat_filter.clear();
            }
            ui.add_space(8.0);
            // Wrapped, so the chips never widen the sidebar past its set
            // width (four chips do not fit in one row at the default 300).
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                if widgets::chip(ui, &palette, "Unread", app.unread_only)
                    .on_hover_text("Show only chats with unread messages")
                    .clicked()
                {
                    app.unread_only = !app.unread_only;
                }
                if widgets::chip(ui, &palette, "Groups", app.groups_only)
                    .on_hover_text("Show group chats")
                    .clicked()
                {
                    app.groups_only = !app.groups_only;
                }
                if widgets::chip(ui, &palette, "Meetings", app.meetings_only)
                    .on_hover_text("Show meeting chats")
                    .clicked()
                {
                    app.meetings_only = !app.meetings_only;
                }
                // Rarely needed, so an icon after the kind chips, and only
                // while there is something hidden (or the filter is on, so it
                // can be turned off). All four then fit one row.
                if (app.hidden_only || app.chats.iter().any(|chat| chat.hidden))
                    && widgets::icon_chip(ui, &palette, Icon::EyeOff, app.hidden_only)
                        .on_hover_text("Hidden chats")
                        .clicked()
                {
                    app.hidden_only = !app.hidden_only;
                }
            });
        });
    ui.add_space(8.0);

    let me = app.me().to_owned();
    let visible: Vec<usize> = app
        .chats
        .iter()
        .enumerate()
        .filter(|(_, chat)| listed(app, chat, &me))
        .map(|(index, _)| index)
        .collect();
    if visible.is_empty() {
        let by_kind = app.groups_only || app.meetings_only;
        let kind = kind_label(app.groups_only, app.meetings_only);
        let kind_text;
        let (title, text) = if !app.chats.is_empty() && !app.chat_filter.trim().is_empty() {
            ("No matches", "No chat or person matches the filter.")
        } else if !app.chats.is_empty() && app.hidden_only {
            ("No hidden chats", "Chats you hide show up here.")
        } else if !app.chats.is_empty() && app.unread_only {
            if by_kind {
                kind_text = format!("No {kind} have unread messages.");
                ("All caught up", kind_text.as_str())
            } else {
                ("All caught up", "No chats have unread messages.")
            }
        } else if !app.chats.is_empty() && by_kind {
            kind_text = format!("You have no {kind} yet.");
            ("Nothing to show", kind_text.as_str())
        } else if app.chats.is_empty() {
            if app.loading.contains(&crate::sync::Resource::Chats) {
                (
                    "Loading chats…",
                    "Your chats appear here as soon as Microsoft answers.",
                )
            } else {
                ("No chats yet", "Chats you start in Teams show up here.")
            }
        } else {
            ("No matches", "No chat or person matches the filter.")
        };
        widgets::empty_state(ui, &palette, Icon::Chat, title, text);
        return;
    }
    let now = crate::time::now();
    let mut actions = Vec::new();
    egui::ScrollArea::vertical()
        .id_salt("chat-list")
        .auto_shrink([false, false])
        .show_rows(ui, theme::ROW_HEIGHT, visible.len(), |ui, range| {
            for index in range {
                let chat = &app.chats[visible[index]];
                let response = row(ui, app, chat, &me, now);
                response.context_menu(|ui| menu(ui, app, chat, &me, &mut actions));
                if response.clicked() {
                    actions.push(Action::OpenConversation(ConversationId::Chat(
                        chat.id.clone(),
                    )));
                }
            }
        });
    app.actions.extend(actions);
}

/// The right-click menu of a chat row.
fn menu(ui: &mut egui::Ui, app: &App, chat: &Chat, me: &str, actions: &mut Vec<Action>) {
    // A fixed width: a context menu offers the whole window as room.
    ui.set_width(240.0);
    let palette = &app.palette;
    let id = chat.id.clone();
    if !chat.is_notes() {
        let (icon, label, action) = if chat.hidden {
            (Icon::Eye, "Unhide", Action::UnhideChat(id.clone()))
        } else {
            (Icon::EyeOff, "Hide", Action::HideChat(id.clone()))
        };
        if menu_item(ui, palette, icon, label) {
            actions.push(action);
            ui.close();
        }
    }
    let (icon, label, action) = if chat.is_unread(me) {
        (Icon::Check, "Mark as read", Action::MarkChatRead(id))
    } else {
        (Icon::Chat, "Mark as unread", Action::MarkChatUnread(id))
    };
    if menu_item(ui, palette, icon, label) {
        actions.push(action);
        ui.close();
    }
    let muted = is_muted(app, chat);
    let (icon, label) = if muted {
        (Icon::Bell, "Unmute notifications")
    } else {
        (Icon::BellOff, "Mute notifications")
    };
    if menu_item(ui, palette, icon, label) {
        actions.push(Action::MuteChat(chat.id.clone(), !muted));
        ui.close();
    }
    if chat.kind == ChatKind::OneOnOne
        && matches!(app.push, crate::realtime::PushStatus::Live)
        && let Some(peer) = chat.face(me)
    {
        {
            let following = app
                .settings
                .notify
                .follow_available
                .contains_key(&peer.user_id);
            let (icon, label) = if following {
                (Icon::BellOff, "Stop notifying when available")
            } else {
                (Icon::Bell, "Notify when available")
            };
            if menu_item(ui, palette, icon, label) {
                actions.push(Action::FollowAvailable {
                    user: peer.user_id.clone(),
                    name: peer.display_name.clone(),
                    on: !following,
                });
                ui.close();
            }
        }
    }
}

fn is_muted(app: &App, chat: &Chat) -> bool {
    app.settings.notify.muted_chats.contains(&chat.id)
}

fn menu_item(ui: &mut egui::Ui, palette: &theme::Palette, icon: Icon, text: &str) -> bool {
    let (rect, response) = ui.allocate_exact_size(vec2(ui.available_width(), 28.0), Sense::click());
    if ui.is_rect_visible(rect) {
        let hovered = response.hovered() || response.has_focus();
        if hovered {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(6), palette.surface_hover);
        }
        let tint = if hovered {
            palette.text
        } else {
            palette.secondary
        };
        icon.image(tint, 16.0).paint_at(
            ui,
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 16.0, rect.center().y),
                egui::Vec2::splat(16.0),
            ),
        );
        ui.painter().text(
            egui::pos2(rect.left() + 32.0, rect.center().y),
            egui::Align2::LEFT_CENTER,
            text,
            theme::regular(13.0),
            palette.text,
        );
    }
    response
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}

fn row(ui: &mut egui::Ui, app: &App, chat: &Chat, me: &str, now: i64) -> egui::Response {
    let palette = &app.palette;
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, theme::ROW_HEIGHT), Sense::click());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let selected = app.open.as_ref() == Some(&ConversationId::Chat(chat.id.clone()));
    let unread = chat.is_unread(me);
    let inner = rect.shrink2(vec2(8.0, 2.0));
    if selected || response.hovered() {
        ui.painter().rect_filled(
            inner,
            CornerRadius::same(8),
            if selected {
                palette.surface_active
            } else {
                palette.surface_hover
            },
        );
    }
    let title = chat.title(me);
    let avatar_rect =
        egui::Rect::from_min_size(inner.left_top() + vec2(8.0, 9.0), egui::Vec2::splat(38.0));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(avatar_rect));
    match chat.kind {
        ChatKind::OneOnOne => {
            let peer = chat.face(me);
            widgets::avatar(
                &mut child,
                app,
                peer.map(|p| p.user_id.as_str()),
                peer.map_or(title.as_str(), |p| p.display_name.as_str()),
                38.0,
                true,
            );
        }
        ChatKind::Group => {
            widgets::avatar(&mut child, app, None, &title, 38.0, false);
        }
        ChatKind::Meeting => {
            child.painter().rect_filled(
                avatar_rect,
                CornerRadius::same(10),
                palette.surface_active,
            );
            Icon::Calendar.image(palette.accent, 18.0).paint_at(
                &child,
                egui::Rect::from_center_size(avatar_rect.center(), egui::Vec2::splat(18.0)),
            );
        }
    }

    let text_left = avatar_rect.right() + 10.0;
    let time = chat
        .preview
        .as_ref()
        .map(|p| crate::time::short_label(p.at, now))
        .unwrap_or_default();
    let time_galley = ui.ctx().fonts_mut(|f| {
        f.layout_no_wrap(
            time,
            if unread {
                theme::semibold(12.0)
            } else {
                theme::regular(12.0)
            },
            if unread { palette.accent } else { palette.dim },
        )
    });
    let time_pos = egui::pos2(
        inner.right() - 10.0 - time_galley.size().x,
        inner.top() + 12.0,
    );
    let title_width = (time_pos.x - text_left - 8.0).max(20.0);
    let title_galley = elided(
        ui,
        &title,
        if unread {
            theme::bold(14.0)
        } else {
            theme::medium(14.0)
        },
        palette.text,
        title_width,
    );
    ui.painter().galley(
        egui::pos2(text_left, inner.top() + 10.0),
        title_galley,
        palette.text,
    );
    ui.painter().galley(time_pos, time_galley, palette.dim);

    // A system event reads in italics, with names but no author.
    let system = chat.preview.as_ref().is_some_and(|p| p.system);
    let preview = chat.preview.as_ref().map(|p| {
        if p.system {
            return p
                .membership
                .as_ref()
                .map_or_else(|| p.text.clone(), |m| m.describe(|id| app.person_name(id)));
        }
        let who = match (&p.author_id, &p.author) {
            (Some(id), _) if id == me => Some("You".to_owned()),
            (_, Some(author)) if chat.kind != ChatKind::OneOnOne => Some(
                author
                    .split_whitespace()
                    .next()
                    .unwrap_or(author)
                    .to_owned(),
            ),
            _ => None,
        };
        match who {
            Some(who) => format!("{who}: {}", p.text),
            None => p.text.clone(),
        }
    });
    let muted = is_muted(app, chat);
    let reserved = if unread || muted { 28.0 } else { 10.0 };
    let preview_width = (inner.right() - text_left - reserved).max(20.0);
    // An unsent draft stands in for the last message, except in the chat
    // that is open.
    let draft = (app.open != Some(crate::model::ConversationId::Chat(chat.id.clone())))
        .then(|| app.draft_preview(&chat.id))
        .flatten();
    let preview_galley = match &draft {
        Some(text) => draft_galley(ui, text, palette, preview_width),
        None => elided_styled(
            ui,
            preview.as_deref().unwrap_or(""),
            theme::regular(13.0),
            if unread {
                palette.text
            } else {
                palette.secondary
            },
            preview_width,
            system,
        ),
    };
    ui.painter().galley(
        egui::pos2(text_left, inner.top() + 31.0),
        preview_galley,
        palette.secondary,
    );
    if unread {
        ui.painter().circle_filled(
            egui::pos2(inner.right() - 16.0, inner.top() + 39.0),
            4.5,
            palette.accent,
        );
    } else if muted {
        Icon::BellOff.image(palette.dim, 14.0).paint_at(
            ui,
            egui::Rect::from_center_size(
                egui::pos2(inner.right() - 16.0, inner.top() + 39.0),
                egui::Vec2::splat(14.0),
            ),
        );
    }
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A single line cut to `width` with an ellipsis.
pub fn elided(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    elided_styled(ui, text, font, color, width, false)
}

/// The preview line of a chat with an unsent draft: "Draft: " in the accent
/// color, then the text, cut to `width`.
fn draft_galley(
    ui: &egui::Ui,
    text: &str,
    palette: &crate::theme::Palette,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = draft_job(text, palette);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    ui.ctx().fonts_mut(|f| f.layout_job(job))
}

fn draft_job(text: &str, palette: &crate::theme::Palette) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    job.append(
        "Draft: ",
        0.0,
        egui::TextFormat::simple(theme::regular(13.0), palette.accent),
    );
    job.append(
        &text.replace('\n', " "),
        0.0,
        egui::TextFormat::simple(theme::regular(13.0), palette.secondary),
    );
    job
}

/// `elided`, optionally in italics (system events in the chat list).
fn elided_styled(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
    italics: bool,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(
        text.replace('\n', " "),
        egui::TextFormat {
            font_id: font,
            color,
            italics,
            ..Default::default()
        },
    );
    job.wrap = egui::text::TextWrapping::truncate_at_width(width);
    ui.ctx().fonts_mut(|f| f.layout_job(job))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Member;

    #[test]
    fn draft_line_leads_with_an_accent_prefix() {
        let palette = crate::theme::Palette::dark();
        let job = draft_job(
            "see you
later",
            &palette,
        );
        assert_eq!(job.text, "Draft: see you later");
        assert_eq!(job.sections.len(), 2);
        assert_eq!(job.sections[0].format.color, palette.accent);
        assert_eq!(job.sections[1].format.color, palette.secondary);
        assert_eq!(
            &job.text[job.sections[0].byte_range.start.0..job.sections[0].byte_range.end.0],
            "Draft: "
        );
    }

    #[test]
    fn filter_matches_titles_and_members() {
        let chat = Chat {
            topic: Some("Launch crew".into()),
            members: vec![Member {
                tenant_id: None,
                user_id: "u-ada".into(),
                display_name: "Ada Lovelace".into(),
                email: None,
            }],
            ..Default::default()
        };
        assert!(matches_filter(&chat, "", "me"));
        assert!(matches_filter(&chat, "LAUNCH", "me"));
        assert!(matches_filter(&chat, "ada", "me"));
        assert!(!matches_filter(&chat, "grace", "me"));
    }

    #[test]
    fn listed_respects_hidden_only() {
        let (backend, _rx) = crate::backend::Backend::recording();
        let mut app = App::with_backend(
            crate::paths::AppDirs::under(&std::env::temp_dir().join("chaat-test-unused")),
            crate::settings::Settings::default(),
            crate::backend::Waker::default(),
            backend,
            false,
        );
        let shown = Chat {
            id: "a".into(),
            ..Default::default()
        };
        let hidden = Chat {
            id: "b".into(),
            hidden: true,
            ..Default::default()
        };
        assert!(listed(&app, &shown, "me"));
        assert!(!listed(&app, &hidden, "me"));
        app.hidden_only = true;
        assert!(!listed(&app, &shown, "me"));
        assert!(listed(&app, &hidden, "me"));
    }

    #[test]
    fn kind_chips_show_every_kind_when_off_and_the_chosen_kinds_when_on() {
        let of = |kind| Chat {
            kind,
            ..Default::default()
        };
        let (one, group, meeting) = (
            of(ChatKind::OneOnOne),
            of(ChatKind::Group),
            of(ChatKind::Meeting),
        );
        let shown =
            |groups, meetings| [&one, &group, &meeting].map(|c| matches_kind(c, groups, meetings));
        assert_eq!(shown(false, false), [true, true, true]);
        assert_eq!(shown(true, false), [false, true, false]);
        assert_eq!(shown(false, true), [false, false, true]);
        assert_eq!(shown(true, true), [false, true, true]);
    }
}
