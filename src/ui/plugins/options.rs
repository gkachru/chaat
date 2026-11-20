//! The options block of a plugin: option rows, secret buffers and the
//! editing helpers. Everything here is the plugin's own text, drawn as
//! such; the words around it are Chaat's (see `mod.rs`).

use crate::app::{App, PluginState};
use crate::model::Action;
use crate::plugin::options;
use crate::plugin::protocol::{OptionKind, OptionSpec, StatusState};
use crate::settings::PluginEntry;
use crate::theme;
use crate::ui::widgets::{self, PointerControls as _};
use std::time::{Duration, Instant};

pub(super) type SecretBuffers = std::collections::HashMap<(String, String), String>;

pub(super) fn secret_buffers_id() -> egui::Id {
    egui::Id::new("plugin-secret-buffers")
}

/// Drops every unsaved secret the person typed. The page calls it when it was
/// not drawn in the frame before; Settings may call it when it closes.
pub fn forget_secret_buffers(ctx: &egui::Context) {
    ctx.data_mut(|d| {
        d.remove_temp::<SecretBuffers>(secret_buffers_id());
        // The text edit's own state (cursor, undo history) can hold the typed
        // text too, so it goes with the buffers.
        for id in d
            .remove_temp::<SecretFields>(secret_fields_id())
            .unwrap_or_default()
        {
            d.remove::<egui::text_edit::TextEditState>(id);
        }
    });
}

/// The ids of the secret fields drawn so far, so their edit state can be
/// dropped even when the buffer is empty.
pub(super) type SecretFields = Vec<egui::Id>;

pub(super) fn secret_fields_id() -> egui::Id {
    egui::Id::new("plugin-secret-fields")
}

pub(super) fn forget_secret_edit_state(ctx: &egui::Context, field: egui::Id) {
    ctx.data_mut(|d| d.remove::<egui::text_edit::TextEditState>(field));
}

/// An unsaved secret does not survive leaving the page: if the page was not
/// drawn in the previous frame, the buffers go. Buffers of plugins or options
/// that no longer exist go too.
pub(super) fn drop_stale_secret_buffers(app: &App, ui: &egui::Ui) {
    let frame = ui.ctx().cumulative_frame_nr();
    let seen = egui::Id::new("plugin-page-last-frame");
    let last = ui.data(|d| d.get_temp::<u64>(seen));
    if last.is_none_or(|last| frame > last.saturating_add(1)) {
        forget_secret_buffers(ui.ctx());
    }
    let live = |plugin: &str, option: &str| {
        app.plugins.get(plugin).is_some_and(|state| {
            state
                .options
                .iter()
                .any(|o| o.id == option && o.kind == OptionKind::Secret)
        })
    };
    ui.data_mut(|d| {
        d.insert_temp(seen, frame);
        if let Some(buffers) = d.get_temp::<SecretBuffers>(secret_buffers_id()) {
            let mut buffers = buffers;
            buffers.retain(|(plugin, option), _| live(plugin, option));
            d.insert_temp(secret_buffers_id(), buffers);
        }
    });
}

pub(super) fn secret_buffer(ui: &egui::Ui, plugin: &str, option: &str) -> String {
    ui.data(|d| d.get_temp::<SecretBuffers>(secret_buffers_id()))
        .and_then(|buffers| {
            buffers
                .get(&(plugin.to_owned(), option.to_owned()))
                .cloned()
        })
        .unwrap_or_default()
}

pub(super) fn set_secret_buffer(ui: &egui::Ui, plugin: &str, option: &str, value: String) {
    ui.data_mut(|d| {
        let mut buffers = d
            .get_temp::<SecretBuffers>(secret_buffers_id())
            .unwrap_or_default();
        let key = (plugin.to_owned(), option.to_owned());
        if value.is_empty() {
            buffers.remove(&key);
        } else {
            buffers.insert(key, value);
        }
        d.insert_temp(secret_buffers_id(), buffers);
    });
}

/// A typing buffer lives in egui memory, per plugin and option.
pub(super) fn buffer_id(plugin: &str, option: &str, what: &str) -> egui::Id {
    egui::Id::new(("plugin-option", what, plugin, option))
}

pub(super) fn labelled(ui: &mut egui::Ui, palette: &theme::Palette, label: &str) {
    ui.label(
        egui::RichText::new(label)
            .font(theme::regular(13.0))
            .color(palette.secondary),
    );
}

/// Chaat's own small marker next to the label of a required option.
pub(super) fn required_marker(ui: &mut egui::Ui, palette: &theme::Palette, option: &OptionSpec) {
    if option.required {
        ui.label(
            egui::RichText::new("Required")
                .font(theme::regular(12.0))
                .color(palette.dim),
        );
    }
}

pub(super) fn label_line(ui: &mut egui::Ui, palette: &theme::Palette, option: &OptionSpec) {
    ui.horizontal(|ui| {
        labelled(ui, palette, &option.label);
        required_marker(ui, palette, option);
    });
}

/// Chaat's own badge on a plugin that still needs required settings.
pub(super) const WORKING_LIMIT: Duration = Duration::from_secs(30);

/// What the option rows of one plugin need: Chaat's state, the plugin's entry
/// and state, and each option's current value (defaults filled in), which
/// `visible_when` reads.
pub(super) struct OptionsCx<'a> {
    pub(super) app: &'a App,
    pub(super) entry: &'a PluginEntry,
    pub(super) state: &'a PluginState,
    pub(super) values: serde_json::Map<String, serde_json::Value>,
    pub(super) now: Instant,
}

impl<'a> OptionsCx<'a> {
    pub(super) fn new(app: &'a App, entry: &'a PluginEntry, state: &'a PluginState) -> Self {
        let stored: serde_json::Map<String, serde_json::Value> = entry
            .options
            .iter()
            .map(|(option, value)| (option.clone(), value.clone()))
            .collect();
        Self {
            app,
            entry,
            state,
            values: options::effective_values(&state.options, &stored),
            now: Instant::now(),
        }
    }

    pub(super) fn visible(&self, spec: &OptionSpec) -> bool {
        options::is_visible(spec, &self.values)
    }

    /// The inline error of the last refused value for an option.
    pub(super) fn error(&self, option: &str) -> Option<&String> {
        self.app
            .plugin_option_errors
            .get(&(self.entry.id.clone(), option.to_owned()))
    }
}

/// The options under their section headings, in the order the sections first
/// appear. Each section has "Reset to defaults" and folds its advanced
/// options into a collapsed "Advanced" area.
pub(super) fn options_block(
    cx: &OptionsCx,
    ui: &mut egui::Ui,
    specs: &[&OptionSpec],
    actions: &mut Vec<Action>,
) {
    let palette = cx.app.palette;
    let mut groups: Vec<(Option<&str>, Vec<&OptionSpec>)> = Vec::new();
    for spec in specs {
        let section = spec.section.as_deref();
        match groups.iter_mut().find(|(name, _)| *name == section) {
            Some((_, members)) => members.push(spec),
            None => groups.push((section, vec![spec])),
        }
    }
    let named = groups.iter().any(|(name, _)| name.is_some());
    for (section, members) in groups {
        let shown: Vec<&OptionSpec> = members
            .iter()
            .copied()
            .filter(|spec| cx.visible(spec))
            .collect();
        if shown.is_empty() {
            continue;
        }
        let resettable = members.iter().any(|spec| {
            !matches!(
                spec.kind,
                OptionKind::Secret | OptionKind::Action | OptionKind::Unsupported { .. }
            )
        });
        // Options without a section sit apart from named ones under Chaat's
        // own heading.
        let heading = section.or(named.then_some("Other settings"));
        ui.add_space(6.0);
        if heading.is_some() || resettable {
            ui.horizontal(|ui| {
                if let Some(heading) = heading {
                    ui.label(
                        egui::RichText::new(heading)
                            .font(theme::semibold(13.0))
                            .color(palette.text),
                    );
                }
                if resettable {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let reset = widgets::text_button(
                            ui,
                            &palette,
                            "Reset to defaults",
                            theme::regular(12.0),
                        );
                        if reset.clicked() {
                            actions.push(Action::ResetPluginSection {
                                plugin: cx.entry.id.clone(),
                                section: section.map(str::to_owned),
                            });
                        }
                    });
                }
            });
        }
        let (advanced, plain): (Vec<&OptionSpec>, Vec<&OptionSpec>) =
            shown.iter().copied().partition(|spec| spec.advanced);
        for spec in plain {
            option_row(cx, ui, spec, actions);
        }
        if !advanced.is_empty() {
            egui::CollapsingHeader::new(
                egui::RichText::new("Advanced")
                    .font(theme::regular(13.0))
                    .color(palette.secondary),
            )
            .id_salt(("plugin-advanced", &cx.entry.id, section))
            .show(ui, |ui| {
                for spec in advanced {
                    option_row(cx, ui, spec, actions);
                }
            });
        }
    }
}

/// The setup view of a plugin: its required options first, then the rest.
pub(super) fn edit_text(
    ui: &mut egui::Ui,
    id: egui::Id,
    saved: &str,
    failed: bool,
    multiline: bool,
    add: impl FnOnce(&mut egui::Ui, &mut String, egui::Id) -> egui::Response,
) -> Option<String> {
    let field = id.with("field");
    let editing = ui
        .data(|d| d.get_temp::<bool>(id.with("editing")))
        .unwrap_or(false);
    let pending = ui.data(|d| d.get_temp::<String>(id.with("pending")));
    if !failed {
        ui.data_mut(|d| d.remove_temp::<String>(id.with("pending")));
    }
    let mut buffer = if editing {
        ui.data(|d| d.get_temp::<String>(id))
            .unwrap_or_else(|| saved.to_owned())
    } else if let Some(text) = pending.filter(|_| failed) {
        text
    } else {
        saved.to_owned()
    };
    let ctrl_enter = multiline
        && editing
        && ui.input_mut(|input| {
            input.consume_key(egui::Modifiers::CTRL, egui::Key::Enter)
                || input.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)
        });
    let escape = ui.input(|input| input.key_pressed(egui::Key::Escape));
    let response = add(ui, &mut buffer, field);
    if ctrl_enter {
        response.surrender_focus();
    }
    let cancelled = response.lost_focus() && escape;
    let commit = (response.lost_focus() && !escape) || ctrl_enter;
    let changed = buffer != saved;
    let focused = response.has_focus() && !ctrl_enter;
    ui.data_mut(|d| {
        if commit || cancelled {
            d.remove_temp::<String>(id);
            d.insert_temp(id.with("editing"), false);
            if commit && changed {
                d.insert_temp(id.with("pending"), buffer.clone());
            }
        } else if focused {
            d.insert_temp(id.with("editing"), true);
            d.insert_temp(id, buffer.clone());
        } else {
            d.insert_temp(id.with("editing"), false);
            d.remove_temp::<String>(id);
        }
    });
    (commit && changed).then_some(buffer)
}

/// A number the way the plugin takes it: whole numbers without a fraction.
/// Text that is no number goes on as text, so the check names the problem.
pub(super) fn number_value(text: &str) -> serde_json::Value {
    match text.trim().parse::<f64>() {
        Ok(number) if number.is_finite() => {
            if number.fract() == 0.0 && number.abs() < 1e15 {
                serde_json::Value::from(number as i64)
            } else {
                serde_json::Number::from_f64(number)
                    .map_or_else(|| serde_json::Value::from(text), serde_json::Value::Number)
            }
        }
        _ => serde_json::Value::from(text),
    }
}

pub(super) fn parse_hex(text: &str) -> Option<egui::Color32> {
    let digits = text.trim().strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some(egui::Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?))
}

pub(super) fn to_hex(color: egui::Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

/// A color option: a swatch that opens egui's picker (the choice is saved
/// when the picker closes) and a field for the hex value.
pub(super) fn color_control(
    cx: &OptionsCx,
    ui: &mut egui::Ui,
    option: &OptionSpec,
    saved: &str,
    actions: &mut Vec<Action>,
) {
    let palette = cx.app.palette;
    let entry = cx.entry;
    let set = |value: String| Action::SetPluginOption {
        plugin: entry.id.clone(),
        option: option.id.clone(),
        value: value.into(),
    };
    let id = buffer_id(&entry.id, &option.id, "color");
    let picked_id = id.with("picked");
    let open_id = id.with("was-open");
    let popup_id = id.with("popup");
    let picked = ui.data(|d| d.get_temp::<String>(picked_id));
    ui.horizontal(|ui| {
        let shown = picked.as_deref().unwrap_or(saved);
        let mut color = parse_hex(shown).unwrap_or(palette.dim);
        let (rect, swatch) = ui.allocate_exact_size(egui::vec2(32.0, 26.0), egui::Sense::click());
        if ui.is_rect_visible(rect) {
            ui.painter().rect_filled(rect, 6.0, color);
            ui.painter().rect_stroke(
                rect,
                6.0,
                egui::Stroke::new(1.0, palette.outline),
                egui::StrokeKind::Inside,
            );
        }
        let swatch = swatch.on_hover_cursor(egui::CursorIcon::PointingHand);
        let mut now_picked = None;
        egui::Popup::menu(&swatch)
            .id(popup_id)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                if egui::color_picker::color_picker_color32(
                    ui,
                    &mut color,
                    egui::color_picker::Alpha::Opaque,
                ) {
                    now_picked = Some(to_hex(color));
                }
            });
        if let Some(hex) = now_picked {
            ui.data_mut(|d| d.insert_temp(picked_id, hex));
        }
        let open = egui::Popup::is_id_open(ui.ctx(), popup_id);
        let was_open = ui.data(|d| d.get_temp::<bool>(open_id)).unwrap_or(false);
        ui.data_mut(|d| d.insert_temp(open_id, open));
        if was_open && !open {
            let chosen = ui.data_mut(|d| d.remove_temp::<String>(picked_id));
            if let Some(hex) = chosen.filter(|hex| hex != saved) {
                actions.push(set(hex));
            }
        }
        let hint = option.placeholder.as_deref().unwrap_or("#rrggbb");
        let failed = cx.error(&option.id).is_some();
        let hex_id = id.with("hex");
        if let Some(text) = edit_text(ui, hex_id, saved, failed, false, |ui, buffer, field| {
            ui.add(
                widgets::text_field(buffer, hint, theme::regular(14.0))
                    .id(field)
                    .desired_width(110.0),
            )
        }) {
            actions.push(set(text));
        }
    });
}

/// The time left until a button that shows "Working…" gives up waiting.
pub(super) fn working_ends_in(cx: &OptionsCx, option: &str) -> Duration {
    let clicked = cx.state.action_started.get(option).copied();
    let status = cx
        .state
        .statuses
        .get(option)
        .filter(|(state, _, _)| *state == StatusState::Working)
        .map(|(_, _, at)| *at);
    let since = clicked.into_iter().chain(status).max().unwrap_or(cx.now);
    WORKING_LIMIT.saturating_sub(cx.now.saturating_duration_since(since))
}

/// Under an option: its help, the error for a refused value, and the
/// plugin's last status for it, quoted as the plugin's words.
pub(super) fn option_footer(cx: &OptionsCx, ui: &mut egui::Ui, option: &OptionSpec) {
    let palette = cx.app.palette;
    let line = |ui: &mut egui::Ui, text: String, color: egui::Color32| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(text)
                    .font(theme::regular(12.0))
                    .color(color),
            )
            .wrap(),
        );
    };
    if let Some(help) = &option.help {
        line(ui, help.clone(), palette.dim);
    }
    if let Some(error) = cx.error(&option.id) {
        line(ui, error.clone(), palette.danger);
    }
    let Some((state, message, _)) = cx.state.statuses.get(&option.id) else {
        return;
    };
    let quoted = message
        .as_ref()
        .map(|message| format!("\u{201C}{message}\u{201D}"));
    let (text, color) = match state {
        StatusState::Ok => (quoted.unwrap_or_else(|| "Done".into()), palette.success),
        StatusState::Error => (quoted.unwrap_or_else(|| "Failed".into()), palette.danger),
        StatusState::Working => {
            if !cx.state.action_working(&option.id, cx.now) {
                return;
            }
            let text = match (quoted, message) {
                (Some(quoted), Some(message)) => working_text(&quoted, message),
                // A button already says "Working…".
                _ if option.kind == OptionKind::Action => return,
                _ => "Working\u{2026}".into(),
            };
            (text, palette.dim)
        }
    };
    line(ui, text, color);
}

/// The quoted working message, with an ellipsis added only when the message
/// does not already end in one.
pub(super) fn working_text(quoted: &str, message: &str) -> String {
    if message.ends_with('\u{2026}') || message.ends_with("...") {
        quoted.to_owned()
    } else {
        format!("{quoted}\u{2026}")
    }
}

pub(super) fn option_row(
    cx: &OptionsCx,
    ui: &mut egui::Ui,
    option: &OptionSpec,
    actions: &mut Vec<Action>,
) {
    let palette = cx.app.palette;
    let entry = cx.entry;
    let current = entry
        .options
        .get(&option.id)
        .or(option.default.as_ref())
        .cloned();
    let set = |value: serde_json::Value| Action::SetPluginOption {
        plugin: entry.id.clone(),
        option: option.id.clone(),
        value,
    };
    let failed = cx.error(&option.id).is_some();
    let hint = option.placeholder.as_deref().unwrap_or(&option.label);
    ui.add_space(2.0);
    match &option.kind {
        OptionKind::Toggle => {
            let mut on = current.as_ref().and_then(|v| v.as_bool()).unwrap_or(false);
            ui.horizontal(|ui| {
                if ui.checkbox_hand(&mut on, &option.label).changed() {
                    actions.push(set(on.into()));
                }
                required_marker(ui, &palette, option);
            });
        }
        OptionKind::Choice => {
            let stored = current.as_ref().and_then(|v| v.as_str());
            let selected =
                stored.and_then(|value| option.choices.iter().find(|c| c.value == value));
            // A stored value the plugin no longer offers is kept and said so.
            let shown = match (selected, stored) {
                (Some(choice), _) => choice.label.as_str(),
                (None, Some(_)) => "(not available)",
                (None, None) => "Not set",
            };
            ui.horizontal(|ui| {
                labelled(ui, &palette, &option.label);
                required_marker(ui, &palette, option);
                egui::ComboBox::from_id_salt(buffer_id(&entry.id, &option.id, "choice"))
                    .selected_text(shown)
                    .show_ui(ui, |ui| {
                        for choice in &option.choices {
                            let on = selected.is_some_and(|s| s.value == choice.value);
                            if ui.selectable_label(on, &choice.label).clicked() && !on {
                                actions.push(set(choice.value.clone().into()));
                            }
                        }
                    });
            });
        }
        OptionKind::Text => {
            let saved = current
                .as_ref()
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            label_line(ui, &palette, option);
            let id = buffer_id(&entry.id, &option.id, "text");
            let edited = edit_text(ui, id, saved, failed, false, |ui, buffer, field| {
                ui.add(widgets::text_field(buffer, hint, theme::regular(14.0)).id(field))
            });
            if let Some(text) = edited {
                actions.push(set(text.into()));
            }
        }
        OptionKind::Multiline => {
            let saved = current
                .as_ref()
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            label_line(ui, &palette, option);
            let id = buffer_id(&entry.id, &option.id, "multiline");
            let edited = edit_text(ui, id, saved, failed, true, |ui, buffer, field| {
                ui.add(
                    egui::TextEdit::multiline(buffer)
                        .id(field)
                        .hint_text(egui::RichText::new(hint).font(theme::regular(14.0)))
                        .font(theme::regular(14.0))
                        .margin(egui::Margin::symmetric(8, 6))
                        .desired_width(f32::INFINITY)
                        .desired_rows(3),
                )
            });
            if let Some(text) = edited {
                actions.push(set(text.into()));
            }
        }
        OptionKind::Number => {
            let saved = current
                .as_ref()
                .and_then(|v| v.as_f64())
                .map(|n| n.to_string())
                .unwrap_or_default();
            label_line(ui, &palette, option);
            let id = buffer_id(&entry.id, &option.id, "number");
            ui.horizontal(|ui| {
                let edited = edit_text(ui, id, &saved, failed, false, |ui, buffer, field| {
                    ui.add(
                        widgets::text_field(buffer, hint, theme::regular(14.0))
                            .id(field)
                            .desired_width(110.0),
                    )
                });
                if let Some(text) = edited {
                    actions.push(set(number_value(&text)));
                }
                if let Some(unit) = &option.unit {
                    ui.label(
                        egui::RichText::new(unit)
                            .font(theme::regular(13.0))
                            .color(palette.dim),
                    );
                }
            });
        }
        OptionKind::Color => {
            let saved = current
                .as_ref()
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            label_line(ui, &palette, option);
            color_control(cx, ui, option, saved, actions);
        }
        OptionKind::Action => {
            let working = cx.state.action_working(&option.id, cx.now);
            let running = cx.state.running;
            let text = if working {
                "Working\u{2026}"
            } else {
                option.label.as_str()
            };
            let response = ui
                .add_enabled_ui(!working && running, |ui| {
                    widgets::secondary_button(ui, &palette, text)
                })
                .inner;
            if response.clicked() {
                actions.push(Action::RunPluginAction {
                    plugin: entry.id.clone(),
                    action: option.id.clone(),
                });
            }
            if !running {
                response.on_disabled_hover_text("The plugin is not running.");
            }
        }
        OptionKind::Secret => {
            let saved = entry.secrets.contains(&option.id);
            label_line(ui, &palette, option);
            ui.label(
                egui::RichText::new(if saved { "Saved" } else { "Not set" })
                    .font(theme::regular(13.0))
                    .color(palette.dim),
            );
            // The stored value is never read back into the interface.
            let mut buffer = secret_buffer(ui, &entry.id, &option.id);
            let field = buffer_id(&entry.id, &option.id, "secret");
            ui.data_mut(|d| {
                let mut fields = d
                    .get_temp::<SecretFields>(secret_fields_id())
                    .unwrap_or_default();
                if !fields.contains(&field) {
                    fields.push(field);
                    d.insert_temp(secret_fields_id(), fields);
                }
            });
            ui.horizontal(|ui| {
                ui.add(
                    widgets::text_field(&mut buffer, "New value", theme::regular(14.0))
                        .id(field)
                        .password(true)
                        .desired_width((ui.available_width() - 150.0).max(80.0)),
                );
                let typed = !buffer.is_empty();
                if ui.add_enabled(typed, egui::Button::new("Save")).clicked() {
                    forget_secret_edit_state(ui.ctx(), field);
                    actions.push(Action::SetPluginSecret {
                        plugin: entry.id.clone(),
                        option: option.id.clone(),
                        value: Some(std::mem::take(&mut buffer)),
                    });
                }
                if ui.add_enabled(saved, egui::Button::new("Clear")).clicked() {
                    buffer.clear();
                    forget_secret_edit_state(ui.ctx(), field);
                    actions.push(Action::SetPluginSecret {
                        plugin: entry.id.clone(),
                        option: option.id.clone(),
                        value: None,
                    });
                }
            });
            set_secret_buffer(ui, &entry.id, &option.id, buffer);
        }
        OptionKind::Unsupported { .. } => {
            labelled(ui, &palette, &option.label);
            ui.label(
                egui::RichText::new("This setting needs a newer Chaat.")
                    .font(theme::regular(13.0))
                    .color(palette.dim),
            );
        }
    }
    option_footer(cx, ui, option);
    if cx.state.action_working(&option.id, cx.now) {
        // One repaint when the wait ends, not a timer that keeps running.
        ui.ctx()
            .request_repaint_after(working_ends_in(cx, &option.id) + Duration::from_millis(50));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn working_text_adds_an_ellipsis_only_when_missing() {
        assert_eq!(
            working_text("\u{201C}Scan\u{201D}", "Scan"),
            "\u{201C}Scan\u{201D}\u{2026}"
        );
        assert_eq!(
            working_text("\u{201C}Scan\u{2026}\u{201D}", "Scan\u{2026}"),
            "\u{201C}Scan\u{2026}\u{201D}"
        );
        assert_eq!(
            working_text("\u{201C}Scan...\u{201D}", "Scan..."),
            "\u{201C}Scan...\u{201D}"
        );
    }
}
