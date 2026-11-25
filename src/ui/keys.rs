//! Keyboard shortcuts. ⌘ is Ctrl on Windows and Linux.

use egui::{Key, Modifiers};

use crate::app::{App, Overlay, PluginsView};
use crate::model::{Action, PluginAction, Section, SettingsPage};

/// Shortcuts and what they do, in groups, for the settings page.
pub const SHORTCUTS: &[(&str, &[(&str, &str)])] = &[
    (
        "Navigation",
        &[
            ("⌘K", "Search chats, channels and messages"),
            ("⌘F", "Find in the open conversation"),
            ("⌘1 – ⌘4", "Chat, Teams, Calendar, Calls"),
            ("Alt ↑ / Alt ↓", "Previous or next conversation"),
            ("⌘N", "New chat"),
            ("⌘⇧A", "Next account"),
        ],
    ),
    (
        "Composer",
        &[
            ("Enter", "Send"),
            ("Shift Enter", "New line"),
            ("↑ in an empty composer", "Edit your last message"),
            ("@ in the composer", "Mention someone"),
            ("Esc", "Cancel reply or edit, close dialogs and settings"),
        ],
    ),
    (
        "View",
        &[
            ("⌘R", "Refresh what you're looking at"),
            ("⌘ + / ⌘ − / ⌘0", "Zoom in, out, reset"),
            ("⌘,", "Settings"),
            ("⌘/", "Keyboard shortcuts"),
            ("⌘⇧P", "Performance overlay"),
        ],
    ),
];

/// A shortcut as this platform writes it: ⌘ on macOS, Ctrl elsewhere.
pub fn platform(keys: &str) -> String {
    if cfg!(target_os = "macos") {
        keys.to_owned()
    } else {
        keys.replace('⌘', "Ctrl ")
    }
}

/// What Esc does in settings. On the Plugins page it steps out of
/// reordering, then out of a plugin's page, before it closes settings. On the
/// add flow it does nothing.
fn settings_escape(app: &App) -> Option<Action> {
    if app.settings_page != Some(SettingsPage::Plugins) {
        return Some(Action::CloseSettings);
    }
    if app.reordering {
        Some(Action::Plugin(PluginAction::EndReorder))
    } else if matches!(app.plugins_view, PluginsView::Plugin(_)) {
        Some(Action::Plugin(PluginAction::BackToPlugins))
    } else if matches!(app.plugins_view, PluginsView::Adding(_)) {
        // Leaving an add flow means Cancel, which asks for no confirm yet
        // stops the plugin: only its buttons do that, never a stray Esc.
        None
    } else {
        Some(Action::CloseSettings)
    }
}

/// The next scope for the account shortcut: Unified, then each account in
/// order, then Unified again. None with fewer than two accounts.
pub(crate) fn next_scope(unified: bool, active: usize, accounts: usize) -> Option<(bool, usize)> {
    if accounts < 2 {
        return None;
    }
    let active = active.min(accounts - 1);
    Some(if unified {
        (false, active)
    } else if active + 1 < accounts {
        (false, active + 1)
    } else {
        (true, active)
    })
}

pub fn handle(app: &mut App, ctx: &egui::Context) {
    let mut actions = Vec::new();
    // Esc in a settings field leaves the field, not settings.
    let typing = ctx.memory(|memory| memory.focused().is_some());
    // Esc in a setup file dialog closes the dialog, not settings.
    let setup_dialog = app.export.open
        || app.export.passphrase.is_some()
        || app.setup.stage != crate::app::SetupStage::Idle;
    let mut find_requested = false;
    ctx.input_mut(|input| {
        let command = Modifiers::COMMAND;
        // ⌘F finds in the open conversation; elsewhere it opens the switcher.
        let in_conversation = app.open.is_some()
            && app.overlay.is_none()
            && app.settings_page.is_none()
            && matches!(app.section, Section::Chats | Section::Teams);
        if in_conversation && input.consume_key(command, Key::F) {
            find_requested = true;
            actions.push(Action::Find(Some(
                app.find
                    .as_ref()
                    .map(|f| f.query.clone())
                    .unwrap_or_default(),
            )));
        }
        if input.consume_key(command, Key::K) || input.consume_key(command, Key::F) {
            actions.push(if app.overlay == Some(Overlay::Switcher) {
                Action::CloseOverlay
            } else {
                Action::OpenSwitcher
            });
        }
        for (key, section) in [
            (Key::Num1, Section::Chats),
            (Key::Num2, Section::Teams),
            (Key::Num3, Section::Calendar),
            (Key::Num4, Section::Calls),
        ] {
            if input.consume_key(command, key) {
                actions.push(Action::Navigate(section));
            }
        }
        if input.consume_key(Modifiers::ALT, Key::ArrowDown) {
            actions.push(Action::Step(1));
        }
        if input.consume_key(Modifiers::ALT, Key::ArrowUp) {
            actions.push(Action::Step(-1));
        }
        if input.consume_key(command, Key::N) {
            actions.push(Action::OpenNewChat);
        }
        if input.consume_key(command | Modifiers::SHIFT, Key::A) && !app.settings.unified_only {
            match next_scope(app.unified, app.active, app.accounts.len()) {
                Some((true, _)) => actions.push(Action::ShowUnified),
                Some((false, i)) => actions.push(Action::ShowAccount(i)),
                None => {}
            }
        }
        if input.consume_key(command, Key::R) {
            actions.push(Action::Refresh);
        }
        if input.consume_key(command, Key::Comma) && app.shows_workspace() {
            actions.push(Action::OpenSettings);
        }
        if input.consume_key(command, Key::Slash) && app.shows_workspace() {
            actions.push(Action::OpenShortcuts);
        }
        if input.consume_key(command | Modifiers::SHIFT, Key::P) {
            actions.push(Action::TogglePerfOverlay);
        }
        let zoom =
            if input.consume_key(command, Key::Plus) || input.consume_key(command, Key::Equals) {
                Some(app.settings.zoom + 0.1)
            } else if input.consume_key(command, Key::Minus) {
                Some(app.settings.zoom - 0.1)
            } else if input.consume_key(command, Key::Num0) {
                Some(1.0)
            } else {
                None
            };
        if let Some(zoom) = zoom {
            app.settings.zoom = (zoom * 10.0).round().clamp(5.0, 30.0) / 10.0;
            app.mark_settings_dirty();
        }
        if app.overlay.is_some() && input.consume_key(Modifiers::NONE, Key::Escape) {
            actions.push(Action::CloseOverlay);
        } else if app.settings_page.is_some()
            && !typing
            && !setup_dialog
            && input.consume_key(Modifiers::NONE, Key::Escape)
        {
            actions.extend(settings_escape(app));
        }
    });
    if find_requested {
        ctx.data_mut(|d| d.insert_temp(super::find::focus_flag(), true));
    }
    // egui's own zoom keys would fight the setting.
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
    app.actions.extend(actions);
}

#[cfg(test)]
mod next_scope_tests {
    use super::next_scope;

    #[test]
    fn cycles_unified_then_accounts() {
        assert_eq!(next_scope(true, 0, 2), Some((false, 0)));
        assert_eq!(next_scope(false, 0, 2), Some((false, 1)));
        assert_eq!(next_scope(false, 1, 2), Some((true, 1)));
        assert_eq!(next_scope(true, 0, 1), None);
        assert_eq!(next_scope(false, 0, 0), None);
    }
}
