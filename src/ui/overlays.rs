//! Dialogs over the window: the quick switcher, new chat, status message,
//! permission requests, join on a phone and image previews.

use egui::{CornerRadius, Frame, Key, Margin, Modifiers, Sense, Stroke, vec2};

use super::widgets::{self, PointerControls as _};
use crate::app::{App, Overlay};
use crate::model::{Action, ConversationId, Member, SearchHit, StatusExpiry};
use crate::theme::{self, Icon};

pub fn show(app: &mut App, ctx: &egui::Context) {
    match app.overlay.clone() {
        Some(Overlay::Switcher) => switcher(app, ctx),
        Some(Overlay::NewChat) => new_chat(app, ctx),
        Some(Overlay::StatusMessage) => status_message(app, ctx),
        Some(Overlay::Consent(scope)) => consent(app, ctx, scope),
        Some(Overlay::JoinOnPhone(url)) => join_on_phone(app, ctx, &url),
        Some(Overlay::ImagePreview(uri)) => image_preview(app, ctx, &uri),
        Some(Overlay::AddPeople(chat)) => add_people(app, ctx, &chat),
        Some(Overlay::LeaveChat(chat)) => leave_chat(app, ctx, &chat),
        None => {}
    }
}

fn image_preview(app: &mut App, ctx: &egui::Context, uri: &str) {
    let palette = app.palette;
    let viewport = ctx.content_rect();
    let width = (viewport.width() - 32.0).clamp(240.0, 960.0);
    let height = (viewport.height() - 152.0).max(120.0);
    let zoom_id = egui::Id::new("image-preview-actual-size").with(uri);
    // Both draft and message images are already in egui's local byte cache.
    // Use decoded dimensions, not the message HTML's thumbnail dimensions.
    let loaded = egui::Image::new(crate::images::original(uri))
        .load_for_size(ctx, vec2(width - 32.0, height));
    let mut actual_size = ctx.data(|d| d.get_temp::<bool>(zoom_id)).unwrap_or(false);
    let mut actions = Vec::new();
    let close = modal(palette, ctx, "image-preview", width, 24.0, |ui| {
        ui.set_height(height + 96.0);
        Frame::new().inner_margin(Margin::same(16)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Image preview")
                        .font(theme::semibold(15.0))
                        .color(palette.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close =
                        widgets::icon_button(ui, &palette, Icon::Close, "Close preview (Esc)");
                    widgets::on_click(&close, &mut actions, Action::CloseOverlay);
                });
            });
            ui.horizontal(|ui| {
                ui.selectable_value(&mut actual_size, false, "Fit");
                ui.selectable_value(&mut actual_size, true, "100%");
                if let Ok(egui::load::TexturePoll::Ready { texture }) = &loaded {
                    ui.label(
                        egui::RichText::new(format!(
                            "{} × {}",
                            texture.size.x as u32, texture.size.y as u32
                        ))
                        .font(theme::regular(12.0))
                        .color(palette.dim),
                    );
                }
            });
            ui.add_space(8.0);
            egui::ScrollArea::both()
                .id_salt(("image-preview-scroll", uri, actual_size))
                .max_height(height)
                .auto_shrink([false, false])
                // Drag the picture to pan around it, as image viewers do; egui
                // only drags to scroll on touch screens by default.
                .scroll_source(egui::scroll_area::ScrollSource::ALL)
                .show_viewport(ui, |ui, viewport| {
                    let available = viewport.size();
                    let Ok(egui::load::TexturePoll::Ready { texture }) = &loaded else {
                        let (canvas, _) = ui.allocate_exact_size(available, Sense::hover());
                        ui.painter().text(
                            canvas.center(),
                            egui::Align2::CENTER_CENTER,
                            if loaded.is_err() {
                                "Image unavailable"
                            } else {
                                "Loading image…"
                            },
                            theme::regular(14.0),
                            palette.dim,
                        );
                        return;
                    };
                    let natural = texture.size;
                    let scale = if actual_size {
                        1.0
                    } else {
                        (available.x / natural.x)
                            .min(available.y / natural.y)
                            .min(1.0)
                    };
                    let size = natural * scale;
                    let (canvas, response) =
                        ui.allocate_exact_size(size.max(available), Sense::hover());
                    // A hand shows the picture can be dragged when it is
                    // larger than the window, and closes while dragging.
                    // `contains_pointer`, not `hovered`: while the scroll
                    // area drags, egui reports nothing else as hovered.
                    if response.contains_pointer() && (size.x > available.x || size.y > available.y)
                    {
                        ui.ctx()
                            .set_cursor_icon(if ui.input(|i| i.pointer.primary_down()) {
                                egui::CursorIcon::Grabbing
                            } else {
                                egui::CursorIcon::Grab
                            });
                    }
                    ui.painter().rect_filled(canvas, 6, palette.window);
                    egui::Image::from_texture(*texture)
                        .paint_at(ui, egui::Rect::from_center_size(canvas.center(), size));
                });
        });
    });
    ctx.data_mut(|d| d.insert_temp(zoom_id, actual_size));
    if close {
        actions.push(Action::CloseOverlay);
    }
    app.actions.extend(actions);
}

fn modal(
    palette: theme::Palette,
    ctx: &egui::Context,
    id: &str,
    width: f32,
    top: f32,
    content: impl FnOnce(&mut egui::Ui),
) -> bool {
    let look = widgets::ModalLook {
        margin: 0,
        shadow: true,
        top: Some(top),
    };
    widgets::modal(palette, ctx, id, width, look, content).should_close()
}

/// A person, chat or channel the switcher can jump to.
struct Target {
    label: String,
    detail: String,
    conversation: ConversationId,
    channel: bool,
    score: i32,
    /// Index of the account the target belongs to.
    account: usize,
}

fn score(text: &str, query: &str) -> Option<i32> {
    let text = text.to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    if text.starts_with(query) {
        return Some(300);
    }
    if text.split_whitespace().any(|word| word.starts_with(query)) {
        return Some(200);
    }
    if text.contains(query) {
        return Some(100);
    }
    // Letters in order (fuzzy).
    let mut chars = text.chars();
    query.chars().all(|q| chars.any(|c| c == q)).then_some(10)
}

fn targets(app: &App, query: &str) -> Vec<Target> {
    let query = query.trim().to_lowercase();
    let mut targets = Vec::new();
    let accounts: Vec<usize> = if app.unified_view() {
        (0..app.accounts.len()).collect()
    } else {
        vec![app.active]
    };
    for account in accounts {
        collect_targets(&app.accounts[account], account, &query, &mut targets);
    }
    targets.sort_by_key(|t| std::cmp::Reverse(t.score));
    targets.truncate(if query.is_empty() { 8 } else { 12 });
    targets
}

fn collect_targets(
    app: &crate::app::Account,
    account: usize,
    query: &str,
    targets: &mut Vec<Target>,
) {
    let me = app.me();
    for (rank, chat) in app.chats.iter().enumerate() {
        let title = chat.title(me);
        let member_names = chat
            .members
            .iter()
            .map(|m| m.display_name.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let best = score(&title, query).or_else(|| score(&member_names, query).map(|s| s / 2));
        if let Some(score) = best {
            targets.push(Target {
                detail: chat
                    .preview
                    .as_ref()
                    .map(|p| p.text.clone())
                    .unwrap_or_default(),
                label: title,
                conversation: ConversationId::Chat(chat.id.clone()),
                channel: false,
                score: score - rank.min(50) as i32,
                account,
            });
        }
    }
    for team in &app.teams {
        for channel in app.channels.get(&team.id).into_iter().flatten() {
            let label = format!("{} › {}", team.name, channel.name);
            if let Some(score) = score(&label, query).or_else(|| score(&channel.name, query)) {
                targets.push(Target {
                    label,
                    detail: channel.description.clone().unwrap_or_default(),
                    conversation: ConversationId::Channel {
                        team: team.id.clone(),
                        channel: channel.id.clone(),
                    },
                    channel: true,
                    score: score - 60,
                    account,
                });
            }
        }
    }
}

/// Message hits with the account each belongs to: this account's local and
/// remote hits, then the other accounts' (`unified_hits`), without repeats.
fn merged_hits(app: &App) -> Vec<(usize, SearchHit)> {
    let mut hits: Vec<(usize, SearchHit)> = Vec::new();
    let own = app
        .switcher
        .hits
        .iter()
        .chain(&app.switcher.remote_hits)
        .map(|h| (app.active, h.clone()));
    for (account, hit) in own.chain(app.switcher.unified_hits.iter().cloned()) {
        if !hits
            .iter()
            .any(|(a, h)| *a == account && h.message_id == hit.message_id)
        {
            hits.push((account, hit));
        }
    }
    hits
}

/// Open an item of `account`: switch to it first when it is not active.
fn open_in(actions: &mut Vec<Action>, active: usize, account: usize, action: Action) {
    if account != active {
        actions.push(Action::SwitchAccount(account));
    }
    actions.push(action);
}

fn switcher(app: &mut App, ctx: &egui::Context) {
    let palette = app.palette;
    let mut actions = Vec::new();
    let mut query = app.switcher.query.clone();
    let found = targets(app, &query);
    let hits = merged_hits(app);
    let active = app.active;
    let looks: Vec<crate::account_look::Look> = if app.unified_view() {
        (0..app.accounts.len())
            .map(|i| app.account_look(i))
            .collect()
    } else {
        Vec::new()
    };
    let total = found.len() + hits.len();
    let mut selected = app.switcher.selected.min(total.saturating_sub(1));
    let mut chosen = false;
    ctx.input_mut(|input| {
        if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
            selected = (selected + 1).min(total.saturating_sub(1));
        }
        if input.consume_key(Modifiers::NONE, Key::ArrowUp) {
            selected = selected.saturating_sub(1);
        }
        if input.consume_key(Modifiers::NONE, Key::Enter) {
            chosen = true;
        }
    });
    let close = modal(palette, ctx, "switcher", 600.0, 90.0, |ui| {
        Frame::new()
            .inner_margin(Margin::symmetric(14, 12))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    theme::icon(ui, Icon::Search, 18.0, palette.dim);
                    let response = ui.add(
                        widgets::text_field(
                            &mut query,
                            "Jump to a chat, channel or person, or search messages",
                            theme::regular(15.0),
                        )
                        .frame(egui::Frame::NONE),
                    );
                    response.request_focus();
                });
            });
        ui.painter().hline(
            ui.max_rect().x_range(),
            ui.cursor().top(),
            Stroke::new(1.0, palette.outline),
        );
        egui::ScrollArea::vertical()
            .max_height(440.0)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                Frame::new()
                    .inner_margin(Margin::symmetric(8, 8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        if !found.is_empty() {
                            group(
                                ui,
                                &palette,
                                if query.trim().is_empty() {
                                    "Recent"
                                } else {
                                    "Conversations"
                                },
                            );
                        }
                        for (index, target) in found.iter().enumerate() {
                            let response = result_row(
                                ui,
                                app,
                                &target.label,
                                &target.detail,
                                if target.channel {
                                    Icon::Hash
                                } else {
                                    Icon::Chat
                                },
                                index == selected,
                                looks.get(target.account),
                            );
                            if response.clicked() || (chosen && index == selected) {
                                open_in(
                                    &mut actions,
                                    active,
                                    target.account,
                                    Action::OpenConversation(target.conversation.clone()),
                                );
                                actions.push(Action::CloseOverlay);
                            }
                            if response.hovered()
                                && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO)
                            {
                                selected = index;
                            }
                        }
                        if query.trim().chars().count() >= 2 {
                            group(ui, &palette, "Messages");
                            if hits.is_empty() {
                                ui.horizontal(|ui| {
                                    ui.add_space(10.0);
                                    let text = if app.switcher.searched == query {
                                        "No messages found."
                                    } else {
                                        "Searching…"
                                    };
                                    ui.label(egui::RichText::new(text).color(palette.dim));
                                });
                            }
                            for (offset, (account, hit)) in hits.iter().enumerate() {
                                let index = found.len() + offset;
                                let title = app.accounts[*account].title_of(&hit.conversation);
                                let detail = format!(
                                    "{} · {} · {}",
                                    hit.author,
                                    crate::time::short_label(hit.at, crate::time::now()),
                                    title
                                );
                                let response = result_row(
                                    ui,
                                    app,
                                    &hit.snippet,
                                    &detail,
                                    Icon::Search,
                                    index == selected,
                                    looks.get(*account),
                                );
                                if response.clicked() || (chosen && index == selected) {
                                    open_in(
                                        &mut actions,
                                        active,
                                        *account,
                                        Action::OpenMessage {
                                            conversation: hit.conversation.clone(),
                                            message: hit.message_id.clone(),
                                        },
                                    );
                                }
                                if response.hovered()
                                    && ui.input(|i| i.pointer.delta() != egui::Vec2::ZERO)
                                {
                                    selected = index;
                                }
                            }
                        }
                    });
            });
        Frame::new().inner_margin(Margin::symmetric(14, 8)).show(ui, |ui| {
            ui.label(
                egui::RichText::new("↑↓ to move · Enter to open · Esc to close · message search covers this computer's cache, then Microsoft Search")
                    .font(theme::regular(11.0))
                    .color(palette.dim),
            );
        });
    });
    if query != app.switcher.query {
        app.switcher.query = query.clone();
        selected = 0;
        if query.trim().chars().count() >= 2 {
            actions.push(Action::SearchMessages(query));
        } else {
            app.switcher.hits.clear();
            app.switcher.remote_hits.clear();
            app.switcher.unified_hits.clear();
        }
    }
    app.switcher.selected = selected;
    if close {
        actions.push(Action::CloseOverlay);
    }
    app.actions.extend(actions);
}

fn group(ui: &mut egui::Ui, palette: &theme::Palette, label: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(10.0);
        widgets::section_label(ui, palette, label);
    });
    ui.add_space(2.0);
}

fn result_row(
    ui: &mut egui::Ui,
    app: &App,
    label: &str,
    detail: &str,
    icon: Icon,
    selected: bool,
    mark: Option<&crate::account_look::Look>,
) -> egui::Response {
    let palette = &app.palette;
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(vec2(width, 44.0), Sense::click());
    if selected || response.hovered() {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(8),
            if selected {
                palette.surface_active
            } else {
                palette.surface_hover
            },
        );
    }
    if selected {
        ui.scroll_to_rect(rect, None);
    }
    icon.image(
        if selected {
            palette.accent
        } else {
            palette.dim
        },
        16.0,
    )
    .paint_at(
        ui,
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 20.0, rect.center().y),
            egui::Vec2::splat(16.0),
        ),
    );
    let left = rect.left() + 40.0;
    let mut right = rect.right() - 12.0;
    if let Some(look) = mark {
        let size = 20.0;
        let mark_rect = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 12.0 - size / 2.0, rect.center().y),
            egui::Vec2::splat(size),
        );
        widgets::paint_account_mark(ui, look, mark_rect);
        right = mark_rect.left() - 8.0;
    }
    let title = super::widgets::elided(ui, label, theme::medium(14.0), palette.text, right - left);
    ui.painter()
        .galley(egui::pos2(left, rect.top() + 5.0), title, palette.text);
    let detail = super::widgets::elided(
        ui,
        detail,
        theme::regular(12.0),
        palette.secondary,
        right - left,
    );
    ui.painter().galley(
        egui::pos2(left, rect.top() + 24.0),
        detail,
        palette.secondary,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Start a chat: pick people from the directory, name a group, start.
fn new_chat(app: &mut App, ctx: &egui::Context) {
    let palette = app.palette;
    let mut actions = Vec::new();
    let mut query = app.new_chat.query.clone();
    let mut topic = app.new_chat.topic.clone();
    let mut picked = app.new_chat.picked.clone();
    let mut selected = app.new_chat.selected;
    let mut start = false;
    let close = modal(palette, ctx, "new-chat", 520.0, 90.0, |ui| {
        Frame::new().inner_margin(Margin::same(20)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("New chat")
                        .font(theme::bold(20.0))
                        .color(palette.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close = widgets::icon_button(ui, &palette, Icon::Close, "Close (Esc)");
                    widgets::on_click(&close, &mut actions, Action::CloseOverlay);
                });
            });
            ui.add_space(10.0);
            if let Some(account) = widgets::account_picker(ui, app, "Start as") {
                actions.push(Action::SwitchAccount(account));
            }
            if people_picker(ui, app, &mut query, &mut picked, &mut selected, &[]) {
                start = true;
            }
            if picked.len() > 1 {
                ui.add_space(10.0);
                ui.label(
                    egui::RichText::new("Group name (optional)")
                        .font(theme::medium(12.0))
                        .color(palette.secondary),
                );
                ui.add(
                    egui::TextEdit::singleline(&mut topic)
                        .hint_text("Launch crew")
                        .desired_width(f32::INFINITY)
                        .margin(Margin::symmetric(10, 6)),
                );
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let label = match picked.len() {
                    0 => "Pick someone to chat with".to_owned(),
                    1 => format!("Chat with {}", picked[0].display_name),
                    n => format!("Start a group chat with {n} people"),
                };
                let button = widgets::accent_button(
                    ui,
                    &palette,
                    !picked.is_empty(),
                    &label,
                    theme::semibold(13.0),
                    32.0,
                );
                if button.clicked() {
                    start = true;
                }
                if picked.len() == 1 {
                    ui.label(
                        egui::RichText::new("An existing chat with this person opens instead.")
                            .font(theme::regular(11.0))
                            .color(palette.dim),
                    );
                }
            });
        });
    });
    if start && !picked.is_empty() {
        actions.push(Action::CreateChat {
            members: picked.clone(),
            topic: (!topic.trim().is_empty()).then(|| topic.clone()),
        });
    }
    app.new_chat.query = query;
    app.new_chat.topic = topic;
    app.new_chat.picked = picked;
    app.new_chat.selected = selected;
    if close {
        actions.push(Action::CloseOverlay);
    }
    app.actions.extend(actions);
}

/// The search box, the people picked so far and the matching people, shared
/// by New chat and Add people. People in `exclude` and the signed-in person
/// are never offered. Returns true when Ctrl/Cmd+Enter asks to go on.
fn people_picker(
    ui: &mut egui::Ui,
    app: &mut App,
    query: &mut String,
    picked: &mut Vec<Member>,
    selected: &mut usize,
    exclude: &[String],
) -> bool {
    let palette = app.palette;
    let before = query.clone();
    // People known from chats first, then the directory's answer.
    let needle = query.trim().to_lowercase();
    let mut found: Vec<Member> = app
        .directory()
        .into_iter()
        .filter(|p| {
            !needle.is_empty()
                && (p.display_name.to_lowercase().contains(&needle)
                    || p.email
                        .as_deref()
                        .is_some_and(|e| e.to_lowercase().contains(&needle)))
        })
        .collect();
    if app.people.query == *query {
        for person in &app.people.results {
            if !found.iter().any(|p| p.user_id == person.user_id) {
                found.push(person.clone());
            }
        }
    }
    found.retain(|p| {
        !picked.iter().any(|q| q.user_id == p.user_id)
            && p.user_id != app.me()
            && !exclude.contains(&p.user_id)
    });
    found.truncate(8);
    *selected = (*selected).min(found.len().saturating_sub(1));
    let mut choose: Option<Member> = None;
    let mut proceed = false;
    ui.ctx().input_mut(|input| {
        if !found.is_empty() {
            if input.consume_key(Modifiers::NONE, Key::ArrowDown) {
                *selected = (*selected + 1) % found.len();
            }
            if input.consume_key(Modifiers::NONE, Key::ArrowUp) {
                *selected = (*selected + found.len() - 1) % found.len();
            }
            if input.consume_key(Modifiers::NONE, Key::Enter) {
                choose = found.get(*selected).cloned();
            }
        } else if !picked.is_empty() && input.consume_key(Modifiers::COMMAND, Key::Enter) {
            proceed = true;
        }
    });
    // The people picked so far.
    if !picked.is_empty() {
        let mut remove = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for (index, person) in picked.iter().enumerate() {
                Frame::new()
                    .fill(palette.surface_hover)
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(Margin {
                        left: 4,
                        right: 2,
                        top: 2,
                        bottom: 2,
                    })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            widgets::avatar(
                                ui,
                                app,
                                Some(&person.user_id),
                                &person.display_name,
                                22.0,
                                false,
                            );
                            ui.label(
                                egui::RichText::new(&person.display_name)
                                    .font(theme::medium(13.0))
                                    .color(palette.text),
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
            picked.remove(index);
        }
        ui.add_space(8.0);
    }
    let response = ui.add(
        egui::TextEdit::singleline(query)
            .hint_text("Type a name or email address")
            .desired_width(f32::INFINITY)
            .margin(Margin::symmetric(10, 8))
            .font(theme::regular(14.0)),
    );
    response.request_focus();
    ui.add_space(6.0);
    for (index, person) in found.iter().enumerate() {
        let (rect, row) = ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
        let row = row.on_hover_cursor(egui::CursorIcon::PointingHand);
        if index == *selected || row.hovered() {
            ui.painter()
                .rect_filled(rect, CornerRadius::same(8), palette.surface_active);
        }
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect.shrink2(vec2(10.0, 0.0)))
                .layout(egui::Layout::left_to_right(egui::Align::Center)),
        );
        widgets::avatar(
            &mut child,
            app,
            Some(&person.user_id),
            &person.display_name,
            30.0,
            true,
        );
        child.add_space(8.0);
        child.vertical(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.label(
                egui::RichText::new(&person.display_name)
                    .font(theme::medium(14.0))
                    .color(palette.text),
            );
            if let Some(email) = &person.email {
                ui.label(
                    egui::RichText::new(email)
                        .font(theme::regular(12.0))
                        .color(palette.dim),
                );
            }
        });
        if row.clicked() {
            choose = Some(person.clone());
        }
    }
    if found.is_empty() && query.trim().chars().count() >= 2 {
        ui.label(egui::RichText::new("Searching your organisation…").color(palette.dim));
    }
    if let Some(person) = choose {
        picked.push(person);
        query.clear();
        *selected = 0;
    }
    if *query != before {
        *selected = 0;
        if query.trim().chars().count() >= 2 {
            app.actions.push(Action::SearchPeople(query.clone()));
        }
    }
    proceed
}

/// Add people to a group chat: pick them, then choose what they see of the
/// chat so far.
fn add_people(app: &mut App, ctx: &egui::Context, chat: &str) {
    let palette = app.palette;
    let mut actions = Vec::new();
    let title = app
        .chats
        .iter()
        .find(|c| c.id == chat)
        .map(|c| c.title(app.me()))
        .unwrap_or_else(|| "this chat".into());
    let exclude: Vec<String> = app
        .group_members(chat)
        .into_iter()
        .map(|m| m.user_id)
        .collect();
    let mut query = app.add_people.query.clone();
    let mut picked = app.add_people.picked.clone();
    let mut selected = app.add_people.selected;
    let mut history_step = app.add_people.history_step;
    let mut history = app.add_people.history;
    let close = modal(palette, ctx, "add-people", 520.0, 90.0, |ui| {
        Frame::new().inner_margin(Margin::same(20)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(format!("Add people to {title}"))
                        .font(theme::bold(18.0))
                        .color(palette.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close = widgets::icon_button(ui, &palette, Icon::Close, "Close (Esc)");
                    widgets::on_click(&close, &mut actions, Action::CloseOverlay);
                });
            });
            ui.add_space(10.0);
            let mut next = false;
            if history_step {
                ui.label(
                    egui::RichText::new("What can the new people see?").color(palette.secondary),
                );
                ui.add_space(6.0);
                ui.radio_hand(&mut history, false, "Don't include chat history");
                ui.radio_hand(&mut history, true, "Include all chat history");
            } else if people_picker(ui, app, &mut query, &mut picked, &mut selected, &exclude) {
                next = true;
            }
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if history_step {
                    if widgets::secondary_button(ui, &palette, "Back").clicked() {
                        history_step = false;
                    }
                    let count = picked.len();
                    let label = if count == 1 {
                        "Add 1 person".to_owned()
                    } else {
                        format!("Add {count} people")
                    };
                    if widgets::primary_button(ui, &palette, &label).clicked() {
                        actions.push(Action::AddMembers {
                            chat: chat.to_owned(),
                            members: picked.clone(),
                            history,
                        });
                    }
                } else {
                    let button = widgets::accent_button(
                        ui,
                        &palette,
                        !picked.is_empty(),
                        "Next",
                        theme::semibold(14.0),
                        34.0,
                    );
                    if button.clicked() {
                        next = true;
                    }
                }
            });
            if next && !picked.is_empty() {
                history_step = true;
            }
        });
    });
    app.add_people.query = query;
    app.add_people.picked = picked;
    app.add_people.selected = selected;
    app.add_people.history_step = history_step;
    app.add_people.history = history;
    if close {
        actions.push(Action::CloseOverlay);
    }
    app.actions.extend(actions);
}

/// Leave a group chat: a confirmation when Chaat holds the permission,
/// otherwise an explanation of what an administrator must approve.
fn leave_chat(app: &mut App, ctx: &egui::Context, chat: &str) {
    let palette = app.palette;
    let mut actions = Vec::new();
    let found = app.chats.iter().find(|c| c.id == chat);
    let title = found
        .map(|c| c.title(app.me()))
        .unwrap_or_else(|| "this chat".into());
    let web_url = found.and_then(|c| c.web_url.clone());
    let allowed = app.has_scope(crate::auth::CHAT_MEMBER_SCOPE);
    let close = modal(palette, ctx, "leave-chat", 460.0, 120.0, |ui| {
        Frame::new().inner_margin(Margin::same(22)).show(ui, |ui| {
            if allowed {
                ui.label(
                    egui::RichText::new("Leave the chat?")
                        .font(theme::bold(18.0))
                        .color(palette.text),
                );
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(format!(
                        "You won't get new messages from {title}. Someone in the chat can add you back."
                    ))
                    .color(palette.text),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if widgets::primary_button(ui, &palette, "  Leave  ").clicked() {
                        actions.push(Action::LeaveChat(chat.to_owned()));
                    }
                    if widgets::secondary_button(ui, &palette, "Cancel").clicked() {
                        actions.push(Action::CloseOverlay);
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    theme::icon(ui, Icon::Lock, 20.0, palette.accent);
                    ui.label(
                        egui::RichText::new("Leaving needs admin approval")
                            .font(theme::bold(18.0))
                            .color(palette.text),
                    );
                });
                ui.add_space(8.0);
                ui.label(
                    egui::RichText::new(
                        "Leaving a chat uses Microsoft Graph's ChatMember.ReadWrite permission, which a tenant administrator must approve.",
                    )
                    .color(palette.text),
                );
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if widgets::primary_button(ui, &palette, "  Request access  ").clicked() {
                        actions.push(Action::RequestScope(crate::auth::CHAT_MEMBER_SCOPE));
                    }
                    if let Some(url) = &web_url
                        && widgets::secondary_button(ui, &palette, "Open in Teams").clicked()
                    {
                        actions.push(Action::OpenUrl(url.clone()));
                    }
                    if widgets::secondary_button(ui, &palette, "Cancel").clicked() {
                        actions.push(Action::CloseOverlay);
                    }
                });
            }
        });
    });
    if close {
        actions.push(Action::CloseOverlay);
    }
    app.actions.extend(actions);
}

fn status_message(app: &mut App, ctx: &egui::Context) {
    let palette = app.palette;
    let mut actions = Vec::new();
    let mut text = app.form_status.clone();
    // Keeps the pick while the dialog is open; the action stores it on Save.
    let mut expiry = ctx.data_mut(|d| {
        *d.get_temp_mut_or_insert_with(egui::Id::new("status-expiry-pick"), || app.status_expiry)
    });
    let close = modal(palette, ctx, "status-message", 440.0, 120.0, |ui| {
        Frame::new().inner_margin(Margin::same(20)).show(ui, |ui| {
            ui.label(
                egui::RichText::new("Status message")
                    .font(theme::bold(18.0))
                    .color(palette.text),
            );
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new("People see it next to your name in Teams and Chaat.")
                    .color(palette.secondary),
            );
            ui.add_space(10.0);
            let response = ui.add(
                egui::TextEdit::multiline(&mut text)
                    .hint_text("Back at 3 pm")
                    .desired_rows(2)
                    .desired_width(f32::INFINITY)
                    .margin(Margin::symmetric(10, 8)),
            );
            response.request_focus();
            ui.add_space(12.0);
            // The label on its own line: beside five chips it wrapped them.
            ui.label(
                egui::RichText::new("Clear after")
                    .font(theme::regular(12.0))
                    .color(palette.secondary),
            );
            ui.add_space(2.0);
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
                for (choice, label) in [
                    (StatusExpiry::Never, "Never"),
                    (StatusExpiry::Today, "Today"),
                    (StatusExpiry::OneHour, "1 hour"),
                    (StatusExpiry::FourHours, "4 hours"),
                    (StatusExpiry::ThisWeek, "This week"),
                ] {
                    if widgets::chip(ui, &palette, label, expiry == choice).clicked() {
                        expiry = choice;
                    }
                }
            });
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &palette, "  Save  ").clicked() {
                    actions.push(Action::SetStatusExpiry(expiry));
                    actions.push(Action::SetStatusMessage(text.clone()));
                }
                if widgets::secondary_button(ui, &palette, "Clear").clicked() {
                    actions.push(Action::SetStatusExpiry(StatusExpiry::Never));
                    actions.push(Action::SetStatusMessage(String::new()));
                }
                if widgets::secondary_button(ui, &palette, "Cancel").clicked() {
                    actions.push(Action::CloseOverlay);
                }
            });
        });
    });
    app.form_status = text;
    if close {
        actions.push(Action::CloseOverlay);
    }
    let pick = egui::Id::new("status-expiry-pick");
    if actions.is_empty() {
        ctx.data_mut(|d| d.insert_temp(pick, expiry));
    } else {
        ctx.data_mut(|d| d.remove_temp::<StatusExpiry>(pick));
    }
    app.actions.extend(actions);
}

/// A meeting's join link as a QR code, for the phone's camera.
fn join_on_phone(app: &mut App, ctx: &egui::Context, url: &str) {
    let palette = app.palette;
    let mut actions = Vec::new();
    let close = modal(palette, ctx, "join-on-phone", 340.0, 100.0, |ui| {
        Frame::new().inner_margin(Margin::same(22)).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Join on your phone")
                        .font(theme::bold(18.0))
                        .color(palette.text),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let close = widgets::icon_button(ui, &palette, Icon::Close, "Close (Esc)");
                    widgets::on_click(&close, &mut actions, Action::CloseOverlay);
                });
            });
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new("Point your phone's camera here; Teams opens the meeting.")
                    .color(palette.secondary),
            );
            ui.add_space(12.0);
            ui.vertical_centered(|ui| qr_code(ui, url, 232.0));
            ui.add_space(12.0);
            if widgets::secondary_button(ui, &palette, "Copy the link").clicked() {
                actions.push(Action::CopyText(url.to_owned()));
            }
        });
    });
    if close {
        actions.push(Action::CloseOverlay);
    }
    app.actions.extend(actions);
}

/// Draws `text` as a QR code: black modules on white, with a quiet zone,
/// whole pixels per module so it stays crisp.
pub fn qr_code(ui: &mut egui::Ui, text: &str, size: f32) {
    let Ok(code) = qrcodegen::QrCode::encode_text(text, qrcodegen::QrCodeEcc::Medium) else {
        ui.label("This link is too long for a QR code.");
        return;
    };
    const QUIET: i32 = 3;
    let modules = code.size() + 2 * QUIET;
    let cell = (size / modules as f32).floor().max(1.0);
    let side = cell * modules as f32;
    let (rect, _) = ui.allocate_exact_size(vec2(side, side), egui::Sense::hover());
    let painter = ui.painter();
    painter.rect_filled(rect, CornerRadius::same(6), egui::Color32::WHITE);
    // One rectangle per run of dark modules in a row.
    for y in 0..code.size() {
        let mut x = 0;
        while x < code.size() {
            if !code.get_module(x, y) {
                x += 1;
                continue;
            }
            let start = x;
            while x < code.size() && code.get_module(x, y) {
                x += 1;
            }
            let min = rect.min + vec2((start + QUIET) as f32 * cell, (y + QUIET) as f32 * cell);
            painter.rect_filled(
                egui::Rect::from_min_size(min, vec2((x - start) as f32 * cell, cell)),
                0.0,
                egui::Color32::BLACK,
            );
        }
    }
}

/// Explains a permission before Chaat asks Microsoft for it.
fn consent(app: &mut App, ctx: &egui::Context, scope: &'static str) {
    let palette = app.palette;
    let mut actions = Vec::new();
    let close = modal(palette, ctx, "consent", 460.0, 120.0, |ui| {
        Frame::new().inner_margin(Margin::same(22)).show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::icon(ui, Icon::Lock, 20.0, palette.accent);
                ui.label(egui::RichText::new("One more permission").font(theme::bold(18.0)).color(palette.text));
            });
            ui.add_space(8.0);
            ui.label(egui::RichText::new(crate::auth::scope_reason(scope)).color(palette.text));
            ui.add_space(6.0);
            ui.label(
                egui::RichText::new(format!(
                    "Chaat asks for Microsoft Graph's {scope} permission only now that you use this. Microsoft's consent page opens in your browser; some organisations need an administrator to approve it."
                ))
                .font(theme::regular(12.0))
                .color(palette.secondary),
            );
            ui.add_space(14.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &palette, "  Continue in browser  ").clicked() {
                    actions.push(Action::RequestScope(scope));
                }
                if widgets::secondary_button(ui, &palette, "Not now").clicked() {
                    actions.push(Action::CloseOverlay);
                }
            });
        });
    });
    if close {
        actions.push(Action::CloseOverlay);
    }
    app.actions.extend(actions);
}

#[cfg(test)]
mod tests {
    use super::score;

    #[test]
    fn prefixes_beat_substrings_beat_fuzzy() {
        assert!(score("ada lovelace", "ada") > score("ada lovelace", "lace"));
        assert!(score("ada lovelace", "love") > score("ada lovelace", "elac"));
        assert!(score("ada lovelace", "alc").is_some());
        assert!(score("ada lovelace", "xyz").is_none());
    }
}
