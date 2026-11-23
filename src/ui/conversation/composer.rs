//! The composer: text, mentions, attachments, drafts and channel consent.

use super::*;

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

pub(super) fn composer(app: &mut App, ui: &mut egui::Ui, conversation: &ConversationId) {
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
            crate::ui::agents::proposal_card(ui, app, conversation, &mut actions);
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

pub(super) fn channel_consent(app: &mut App, ui: &mut egui::Ui, conversation: &ConversationId) {
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
