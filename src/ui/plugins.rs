//! Settings › Plugins: the plugins Chaat launches. The words about what a
//! plugin is are Chaat's; the plugin's own name and description show apart.

use super::settings::{group, note};
use super::widgets::{self, PointerControls as _};
use crate::app::{App, PluginState};
use crate::model::Action;
use crate::plugin::options;
use crate::plugin::protocol::{OptionKind, OptionSpec, StatusState};
use crate::plugin::registry;
use crate::settings::PluginEntry;
use crate::theme;
use std::time::{Duration, Instant};

pub fn page(app: &App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    drop_stale_secret_buffers(app, ui);
    if let Some(entry) = app
        .plugin_setup
        .as_deref()
        .and_then(|id| app.settings.plugins.iter().find(|e| e.id == id))
    {
        setup_view(app, ui, entry, actions);
        return;
    }
    // Chaat's own words, whatever a plugin says about itself.
    note(
        ui,
        &palette,
        "Plugins are separate programs you install, not part of Chaat. Chaat starts the ones you turn on and shows what they report. It never sends a plugin your messages or sign-in tokens. Check what a plugin does before you use it.",
    );
    group(ui, &palette, "Plugins");
    if app.settings.plugins.is_empty() {
        note(ui, &palette, "No plugins added.");
    }
    let owner = app.call_alerts_owner();
    for entry in &app.settings.plugins {
        let state = app.plugins.get(&entry.id);
        ui.add_space(6.0);
        heading(app, ui, entry);
        if let Some(description) = state.and_then(|s| s.info.description.as_ref()) {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(description)
                        .font(theme::regular(13.0))
                        .color(palette.secondary),
                )
                .wrap(),
            );
        }
        if let Some(url) = state.and_then(|s| s.info.docs_url.as_ref()) {
            // The host shows next to the link, so the person sees where it
            // leads before clicking.
            ui.horizontal(|ui| {
                if ui.link("About this plugin").clicked() {
                    actions.push(Action::OpenUrl(url.clone()));
                }
                if let Some(host) = link_host(url) {
                    ui.label(
                        egui::RichText::new(host)
                            .font(theme::regular(12.0))
                            .color(palette.dim),
                    );
                }
            });
        }
        ui.add(
            egui::Label::new(
                egui::RichText::new(entry.path.display().to_string())
                    .font(theme::regular(12.0))
                    .color(palette.dim),
            )
            .truncate(),
        );
        status(app, ui, entry, state);
        if !app.plugin_missing_required(&entry.id).is_empty() {
            ui.horizontal_wrapped(|ui| {
                needs_setup_badge(ui, &palette);
                if widgets::secondary_button(ui, &palette, "Set up").clicked() {
                    actions.push(Action::OpenPluginSetup(entry.id.clone()));
                }
            });
        }
        if let Some(hash) = state.and_then(|s| s.file_changed.as_deref()) {
            file_changed_notice(app, ui, entry, hash, actions);
        }
        if let Some(state) = state
            && !state.pending_needs.is_empty()
        {
            pending_notice(app, ui, entry, &state.pending_needs, actions);
        }
        receives_line(app, ui, entry);
        let offers_alerts = state.is_some_and(|s| s.offers.contains(&registry::CALL_ALERTS));
        if offers_alerts && entry.enabled && owner.as_deref().is_some_and(|o| o != entry.id) {
            note(
                ui,
                &palette,
                "Another plugin provides call alerts, so this one's rings are ignored.",
            );
        }
        ui.horizontal(|ui| {
            let mut enabled = entry.enabled;
            if ui.checkbox_hand(&mut enabled, "Turn on").changed() {
                actions.push(Action::SetPluginEnabled(entry.id.clone(), enabled));
            }
            if widgets::secondary_button(ui, &palette, "Remove").clicked() {
                actions.push(Action::RemovePlugin(entry.id.clone()));
            }
        });
        if let Some(state) = state {
            let cx = OptionsCx::new(app, entry, state);
            let all: Vec<&OptionSpec> = state.options.iter().collect();
            options_block(&cx, ui, &all, actions);
            if state.running && app.plugin_receives(&entry.id).contains(&"accounts") {
                account_rows(app, ui, entry, state, actions);
            }
        }
    }
    ui.add_space(10.0);
    if widgets::secondary_button(ui, &palette, "Add plugin\u{2026}").clicked() {
        actions.push(Action::AddPlugin);
    }
}

/// Chaat's words for what one kind of data is.
fn need_words(need: &str) -> &'static str {
    match need {
        "accounts" => "your accounts' IDs and sign-in names",
        "presence" => "your presence (availability and activity)",
        "options" => "its own settings below",
        _ => "something Chaat does not know",
    }
}

/// "a", "a and b", "a, b and c".
fn join_plainly(items: &[&str]) -> String {
    match items {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [init @ .., last] => format!("{} and {last}", init.join(", ")),
    }
}

/// What the plugin gets from Chaat now, in Chaat's words.
fn receives_line(app: &App, ui: &mut egui::Ui, entry: &PluginEntry) {
    let palette = app.palette;
    let words: Vec<&str> = app
        .plugin_receives(&entry.id)
        .into_iter()
        .map(need_words)
        .collect();
    let text = if words.is_empty() {
        "nothing".to_owned()
    } else {
        join_plainly(&words)
    };
    ui.add(
        egui::Label::new(
            egui::RichText::new(format!("Receives: {text}"))
                .font(theme::regular(13.0))
                .color(palette.secondary),
        )
        .wrap(),
    );
}

/// A boxed notice that stands apart from the plugin's own text.
fn notice(ui: &mut egui::Ui, palette: &theme::Palette, add_contents: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(palette.surface)
        .stroke(egui::Stroke::new(1.0, palette.warning))
        .corner_radius(6)
        .inner_margin(egui::Margin::same(10))
        .show(ui, add_contents);
    ui.add_space(4.0);
}

/// The needs the plugin declared that the person has not answered yet.
fn pending_notice(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    pending: &[&'static str],
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    notice(ui, &palette, |ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!(
                    "Plugin \u{201C}{}\u{201D} asks for:",
                    app.plugin_name(&entry.id)
                ))
                .font(theme::medium(13.0))
                .color(palette.text),
            )
            .wrap(),
        );
        for need in pending {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(need_words(need))
                        .font(theme::regular(13.0))
                        .color(palette.secondary),
                )
                .wrap(),
            );
        }
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if widgets::primary_button(ui, &palette, "Allow").clicked() {
                actions.push(Action::ApprovePluginNeeds(entry.id.clone()));
            }
            if widgets::secondary_button(ui, &palette, "Don't allow").clicked() {
                actions.push(Action::DenyPluginNeeds(entry.id.clone()));
            }
        });
    });
}

/// The plugin's file no longer matches the one the person added.
fn file_changed_notice(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    hash: &str,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    notice(ui, &palette, |ui| {
        ui.add(
            egui::Label::new(
                egui::RichText::new(
                    "The plugin file changed since you added it. Run the new file only if you trust it.",
                )
                .font(theme::regular(13.0))
                .color(palette.text),
            )
            .wrap(),
        );
        let short: String = hash.chars().take(12).collect();
        ui.label(
            egui::RichText::new(format!("New file fingerprint: {short}"))
                .font(theme::regular(12.0))
                .color(palette.dim),
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if widgets::secondary_button(ui, &palette, "Run the new file").clicked() {
                actions.push(Action::TrustPluginFile(entry.id.clone()));
            }
            if widgets::secondary_button(ui, &palette, "Keep it off").clicked() {
                actions.push(Action::KeepPluginOff(entry.id.clone()));
            }
        });
    });
}

/// The host of an `https://` link: no scheme, user info, port or path.
fn link_host(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    let host = if authority.starts_with('[') {
        authority.split_inclusive(']').next()?
    } else {
        authority.split(':').next()?
    };
    (!host.is_empty()).then(|| host.to_owned())
}

/// The plugin's own name, quoted so it reads as the plugin's, not Chaat's.
fn heading(app: &App, ui: &mut egui::Ui, entry: &PluginEntry) {
    let palette = app.palette;
    ui.add(
        egui::Label::new(
            egui::RichText::new(format!(
                "Plugin: \u{201C}{}\u{201D}",
                app.plugin_name(&entry.id)
            ))
            .font(theme::medium(13.0))
            .color(palette.text),
        )
        .truncate(),
    );
}

fn status(app: &App, ui: &mut egui::Ui, entry: &PluginEntry, state: Option<&PluginState>) {
    let palette = app.palette;
    let (status, color) = if !entry.enabled {
        ("Off", palette.dim)
    } else if state.is_some_and(|s| s.running) {
        ("Running", palette.secondary)
    } else if let Some(message) = state.and_then(|s| s.message.as_deref()) {
        (message, palette.warning)
    } else {
        ("Not running", palette.warning)
    };
    ui.add(
        egui::Label::new(
            egui::RichText::new(status)
                .font(theme::regular(13.0))
                .color(color),
        )
        .wrap(),
    );
}

/// Typed but unsaved secrets, by plugin and option, in one egui memory slot so
/// they can all be dropped at once.
type SecretBuffers = std::collections::HashMap<(String, String), String>;

fn secret_buffers_id() -> egui::Id {
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
type SecretFields = Vec<egui::Id>;

fn secret_fields_id() -> egui::Id {
    egui::Id::new("plugin-secret-fields")
}

fn forget_secret_edit_state(ctx: &egui::Context, field: egui::Id) {
    ctx.data_mut(|d| d.remove::<egui::text_edit::TextEditState>(field));
}

/// An unsaved secret does not survive leaving the page: if the page was not
/// drawn in the previous frame, the buffers go. Buffers of plugins or options
/// that no longer exist go too.
fn drop_stale_secret_buffers(app: &App, ui: &egui::Ui) {
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

fn secret_buffer(ui: &egui::Ui, plugin: &str, option: &str) -> String {
    ui.data(|d| d.get_temp::<SecretBuffers>(secret_buffers_id()))
        .and_then(|buffers| {
            buffers
                .get(&(plugin.to_owned(), option.to_owned()))
                .cloned()
        })
        .unwrap_or_default()
}

fn set_secret_buffer(ui: &egui::Ui, plugin: &str, option: &str, value: String) {
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
fn buffer_id(plugin: &str, option: &str, what: &str) -> egui::Id {
    egui::Id::new(("plugin-option", what, plugin, option))
}

fn labelled(ui: &mut egui::Ui, palette: &theme::Palette, label: &str) {
    ui.label(
        egui::RichText::new(label)
            .font(theme::regular(13.0))
            .color(palette.secondary),
    );
}

/// Chaat's own small marker next to the label of a required option.
fn required_marker(ui: &mut egui::Ui, palette: &theme::Palette, option: &OptionSpec) {
    if option.required {
        ui.label(
            egui::RichText::new("Required")
                .font(theme::regular(12.0))
                .color(palette.dim),
        );
    }
}

fn label_line(ui: &mut egui::Ui, palette: &theme::Palette, option: &OptionSpec) {
    ui.horizontal(|ui| {
        labelled(ui, palette, &option.label);
        required_marker(ui, palette, option);
    });
}

/// Chaat's own badge on a plugin that still needs required settings.
fn needs_setup_badge(ui: &mut egui::Ui, palette: &theme::Palette) {
    egui::Frame::new()
        .stroke(egui::Stroke::new(1.0, palette.warning))
        .corner_radius(10)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(
                egui::RichText::new("Needs setup")
                    .font(theme::medium(12.0))
                    .color(palette.warning),
            );
        });
}

/// How long a button shows "Working…" without an answer. The same limit as
/// `PluginState::action_working`, which decides; this only schedules the
/// repaint that ends the wait.
const WORKING_LIMIT: Duration = Duration::from_secs(30);

/// What the option rows of one plugin need: Chaat's state, the plugin's entry
/// and state, and each option's current value (defaults filled in), which
/// `visible_when` reads.
struct OptionsCx<'a> {
    app: &'a App,
    entry: &'a PluginEntry,
    state: &'a PluginState,
    values: serde_json::Map<String, serde_json::Value>,
    now: Instant,
}

impl<'a> OptionsCx<'a> {
    fn new(app: &'a App, entry: &'a PluginEntry, state: &'a PluginState) -> Self {
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

    fn visible(&self, spec: &OptionSpec) -> bool {
        options::is_visible(spec, &self.values)
    }

    /// The inline error of the last refused value for an option.
    fn error(&self, option: &str) -> Option<&String> {
        self.app
            .plugin_option_errors
            .get(&(self.entry.id.clone(), option.to_owned()))
    }
}

/// The options under their section headings, in the order the sections first
/// appear. Each section has "Reset to defaults" and folds its advanced
/// options into a collapsed "Advanced" area.
fn options_block(
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
fn setup_view(app: &App, ui: &mut egui::Ui, entry: &PluginEntry, actions: &mut Vec<Action>) {
    let palette = app.palette;
    group(ui, &palette, "Set up");
    heading(app, ui, entry);
    note(
        ui,
        &palette,
        "Fill in what this plugin needs. Required settings come first. You can change all of them later in Settings \u{203A} Plugins.",
    );
    if let Some(state) = app.plugins.get(&entry.id) {
        let cx = OptionsCx::new(app, entry, state);
        let required: Vec<&OptionSpec> = state
            .options
            .iter()
            .filter(|spec| spec.required && cx.visible(spec))
            .collect();
        let rest: Vec<&OptionSpec> = state.options.iter().filter(|spec| !spec.required).collect();
        if !required.is_empty() {
            group(ui, &palette, "Required");
            for spec in required {
                option_row(&cx, ui, spec, actions);
            }
        }
        if rest.iter().any(|spec| cx.visible(spec)) {
            group(ui, &palette, "More settings");
            options_block(&cx, ui, &rest, actions);
        }
    } else {
        note(
            ui,
            &palette,
            "This plugin has not described its settings yet.",
        );
    }
    ui.add_space(10.0);
    if widgets::primary_button(ui, &palette, "Done").clicked() {
        actions.push(Action::ClosePluginSetup);
    }
}

/// Edits one piece of text with a buffer in egui memory. It commits on Enter
/// or when the field loses focus (Ctrl+Enter for `multiline`, where Enter
/// adds a line) and returns the text then, if it changed; Escape drops the
/// edit. The buffer is cleared on commit. A committed value the app refused
/// (`failed`) stays in the field until the person edits again, so the error
/// under it still refers to what they typed.
fn edit_text(
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
fn number_value(text: &str) -> serde_json::Value {
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

fn parse_hex(text: &str) -> Option<egui::Color32> {
    let digits = text.trim().strip_prefix('#')?;
    if digits.len() != 6 || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |at: usize| u8::from_str_radix(&digits[at..at + 2], 16).ok();
    Some(egui::Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?))
}

fn to_hex(color: egui::Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

/// A color option: a swatch that opens egui's picker (the choice is saved
/// when the picker closes) and a field for the hex value.
fn color_control(
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
fn working_ends_in(cx: &OptionsCx, option: &str) -> Duration {
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
fn option_footer(cx: &OptionsCx, ui: &mut egui::Ui, option: &OptionSpec) {
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
            let text = match quoted {
                Some(quoted) => format!("{quoted}\u{2026}"),
                // A button already says "Working…".
                None if option.kind == OptionKind::Action => return,
                None => "Working\u{2026}".into(),
            };
            (text, palette.dim)
        }
    };
    line(ui, text, color);
}

fn option_row(cx: &OptionsCx, ui: &mut egui::Ui, option: &OptionSpec, actions: &mut Vec<Action>) {
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

/// One row per Chaat account: whether the plugin listens for it or waits for
/// a sign-in, with a button to ask for a fresh code.
fn account_rows(
    app: &App,
    ui: &mut egui::Ui,
    entry: &PluginEntry,
    state: &PluginState,
    actions: &mut Vec<Action>,
) {
    let palette = app.palette;
    for account in app.plugin_accounts() {
        let waiting = state
            .sign_in
            .iter()
            .any(|s| s.account.as_deref() == Some(account.id.as_str()));
        let listening = state.listening.contains(&account.id);
        let name = if account.username.is_empty() {
            account.id.clone()
        } else {
            account.username.clone()
        };
        ui.horizontal(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(name)
                        .font(theme::regular(13.0))
                        .color(palette.text),
                )
                .truncate(),
            );
            let (text, color) = if listening {
                ("Listening for calls", palette.success)
            } else if waiting {
                ("Waiting for sign-in", palette.warning)
            } else {
                ("Connecting\u{2026}", palette.dim)
            };
            ui.label(
                egui::RichText::new(text)
                    .font(theme::regular(12.0))
                    .color(color),
            );
            if waiting && widgets::secondary_button(ui, &palette, "New code").clicked() {
                actions.push(Action::PluginSignIn {
                    plugin: entry.id.clone(),
                    account: account.id.clone(),
                });
            }
        });
    }
}
