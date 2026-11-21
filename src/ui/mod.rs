//! The window: navigation rail, sidebar, the open conversation or page, and
//! overlays. Every view reads [`App`] and pushes actions.

pub mod calendar;
pub mod callbar;
pub mod calls;
pub mod chat_list;
pub mod conversation;
pub mod keys;
pub mod onboarding;
pub mod overlays;
pub mod plugins;
pub mod rail;
pub mod rich;
pub mod settings;
pub mod setup_settings;
pub mod signin;
pub mod teams;
pub mod widgets;

use egui::{Align2, CornerRadius, Frame, Margin, Stroke, vec2};

use crate::app::App;
use crate::model::{Action, Section, ToastKind};
use crate::theme::{self, Icon};

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    keys::handle(app, &ctx);
    if matches!(app.update, crate::updates::Status::Ready(_)) {
        egui::Panel::bottom("update-ready")
            .frame(
                Frame::new()
                    .fill(app.palette.surface)
                    .inner_margin(Margin::symmetric(16, 8)),
            )
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.label("An app update is ready.");
                    if ui.button("Restart to update").clicked() {
                        app.actions.push(Action::InstallUpdate);
                    }
                });
            });
    }
    if app.settings_page.is_none() {
        settings::closed(ui.ctx());
    }
    if app.shows_workspace() && !app.demo {
        session_banner(app, ui);
    }
    if app.shows_workspace() && app.docked.is_some() {
        docked(app, ui);
    } else if app.shows_workspace() {
        rail::show(app, ui);
        // Settings take the place of the sidebar and the content.
        if app.settings_page.is_some() {
            settings::nav(app, ui);
        } else {
            sidebar(app, ui);
        }
        if app.call.is_some() {
            callbar::panel(app, ui, false);
        }
        if app.settings_page.is_some() {
            settings::page(app, ui);
        } else {
            main(app, ui);
        }
    } else {
        signin::show(app, ui);
    }
    // Windows and Linux with Chaat's own title bar: its buttons over the
    // header's right end, and resizing from the window's edges.
    fastframe_titlebar::controls(
        ui,
        &fastframe_titlebar::Style {
            icon: app.palette.secondary,
            button: app.palette.surface_hover,
            header_height: theme::HEADER_HEIGHT,
        },
        &fastframe_titlebar::Labels::default(),
    );
    fastframe_titlebar::resize(ui);
    drop_files(app, &ctx);
    overlays::show(app, &ctx);
    if app.shows_workspace() && !app.demo {
        setup_settings::import_overlay(app, &ctx);
    }
    toasts(app, &ctx);
    rings(app, &ctx);
    if app.settings.perf_overlay {
        perf(app, &ctx);
    }
    if !app.demo {
        widgets::request_photos(app, &ctx);
    }
}

/// Above the cached workspace while its account needs to sign in again: why,
/// and the way back.
fn session_banner(app: &mut App, ui: &mut egui::Ui) {
    use crate::backend::AuthStatus;
    let username = app.username();
    let (text, buttons): (String, &[&str]) = match (&app.auth, app.reauth.is_some()) {
        (AuthStatus::Expired { reason, .. }, _) => (
            if username.is_empty() {
                reason.clone()
            } else {
                format!("{username}: {reason}")
            },
            &["Sign in again"],
        ),
        (AuthStatus::WaitingForBrowser { .. }, true) => (
            "Finish signing in in your browser. New messages arrive once you do.".into(),
            &["Open the page again", "Cancel"],
        ),
        (AuthStatus::DeviceCode { .. } | AuthStatus::Finishing, true) => {
            ("Signing in…".into(), &["Cancel"])
        }
        _ => return,
    };
    let palette = app.palette;
    egui::Panel::top("session-banner")
        .frame(
            Frame::new()
                .fill(palette.surface)
                .inner_margin(Margin::symmetric(16, 8)),
        )
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                theme::icon(ui, Icon::Alert, 16.0, palette.warning);
                ui.label(egui::RichText::new(text).color(palette.text));
                for label in buttons {
                    let clicked = if *label == "Sign in again" {
                        widgets::primary_button(ui, &palette, label).clicked()
                    } else {
                        widgets::secondary_button(ui, &palette, label).clicked()
                    };
                    if !clicked {
                        continue;
                    }
                    match (*label, &app.auth) {
                        ("Sign in again", _) => app.actions.push(Action::SignIn),
                        ("Open the page again", AuthStatus::WaitingForBrowser { url }) => {
                            app.actions.push(Action::OpenUrl(url.clone()));
                        }
                        _ => app.actions.push(Action::CancelSignIn),
                    }
                }
            });
        });
}

/// Files dropped on the window go to the open conversation's draft; while
/// they hover, a hint says where they will go.
fn drop_files(app: &mut App, ctx: &egui::Context) {
    let (dropped, hovering) = ctx.input(|input| {
        let dropped: Vec<std::path::PathBuf> = input
            .raw
            .dropped_files
            .iter()
            .map(|file| file.path().to_path_buf())
            .collect();
        (dropped, !input.raw.hovered_files.is_empty())
    });
    // A setup file goes to the import (welcome screen or import overlay);
    // everything else is a chat attachment.
    let importing = !app.shows_workspace() || app.setup.stage != crate::app::SetupStage::Idle;
    let (setups, files): (Vec<_>, Vec<_>) = dropped
        .into_iter()
        .partition(|path| importing && crate::app::is_setup_file(path));
    if let Some(setup) = setups.into_iter().next() {
        app.actions.push(Action::DroppedSetupFile(setup));
    }
    if !files.is_empty() {
        app.actions.push(Action::AddFiles(files));
    }
    if hovering && app.open.is_some() && app.shows_workspace() {
        let palette = app.palette;
        let title = app
            .open
            .as_ref()
            .map(|open| app.title_of(open))
            .unwrap_or_default();
        egui::Area::new(egui::Id::new("chaat-drop"))
            .order(egui::Order::Foreground)
            .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .interactable(false)
            .show(ctx, |ui| {
                Frame::new()
                    .fill(palette.surface)
                    .stroke(Stroke::new(2.0, palette.accent))
                    .corner_radius(CornerRadius::same(14))
                    .inner_margin(Margin::symmetric(28, 18))
                    .show(ui, |ui| {
                        ui.label(
                            egui::RichText::new(format!("Drop to send to {title}"))
                                .font(theme::semibold(15.0))
                                .color(palette.text),
                        );
                    });
            });
    }
}

/// Beside a call window: the call bar on top, then the call's chat (or the
/// open conversation), with no rail or sidebar.
fn docked(app: &mut App, ui: &mut egui::Ui) {
    callbar::panel(app, ui, true);
    let palette = app.palette;
    let chat = app
        .call
        .as_ref()
        .and_then(|call| call.chat.clone())
        .or_else(|| app.open.clone());
    egui::CentralPanel::no_frame()
        .frame(Frame::new().fill(palette.window))
        .show(ui, |ui| match chat {
            Some(chat) => conversation::show(app, ui, chat),
            None => widgets::empty_state(
                ui,
                &palette,
                Icon::Chat,
                "No chat for this call",
                "A meeting's chat shows here once Chaat finds it. Restore the window to see all your chats.",
            ),
        });
}

/// Left padding for a header that sits under the macOS traffic lights.
pub fn header_inset(ctx: &egui::Context, x: f32) -> f32 {
    (fastframe_titlebar::footprint(ctx, theme::HEADER_HEIGHT).left - x).max(0.0)
}

/// Right padding for a header at the window's right edge, beside the drawn
/// title bar buttons on Windows and Linux.
pub fn header_right_inset(ctx: &egui::Context) -> f32 {
    fastframe_titlebar::footprint(ctx, theme::HEADER_HEIGHT).right
}

fn sidebar(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let response = egui::Panel::left("sidebar")
        .resizable(true)
        .default_size(app.settings.sidebar_width)
        .size_range(220.0..=520.0)
        .show_separator_line(false)
        .frame(Frame::new().fill(palette.panel))
        .show(ui, |ui| {
            let header = egui::Rect::from_min_size(
                ui.max_rect().min,
                vec2(ui.max_rect().width(), theme::HEADER_HEIGHT),
            );
            widgets::window_drag(ui, header, "sidebar-drag");
            let inset = header_inset(ui.ctx(), theme::RAIL_WIDTH);
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(header.shrink2(vec2(12.0, 0.0))),
                |ui| {
                    ui.horizontal_centered(|ui| {
                        ui.add_space(inset + 4.0);
                        ui.label(
                            egui::RichText::new(app.section.label())
                                .font(theme::bold(18.0))
                                .color(palette.text),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let refresh =
                                widgets::icon_button(ui, &palette, Icon::Refresh, "Refresh (⌘R)");
                            widgets::on_click(&refresh, &mut app.actions, Action::Refresh);
                            if app.section == Section::Chats {
                                let search = widgets::icon_button(
                                    ui,
                                    &palette,
                                    Icon::Search,
                                    "Search and jump (⌘K)",
                                );
                                widgets::on_click(&search, &mut app.actions, Action::OpenSwitcher);
                                let new_chat = widgets::icon_button(
                                    ui,
                                    &palette,
                                    Icon::Compose,
                                    "New chat (⌘N)",
                                );
                                widgets::on_click(&new_chat, &mut app.actions, Action::OpenNewChat);
                            }
                            let busy = !app.loading.is_empty();
                            if busy {
                                widgets::loading(ui, palette.dim, 14.0);
                            } else if app.unreachable {
                                // Quiet: a dim marker, not an error. Any
                                // later fetch that gets through clears it.
                                let offline = widgets::icon_button(
                                    ui,
                                    &palette,
                                    Icon::Offline,
                                    "Can't reach Microsoft right now. Chaat tries again when you come back to the window, or click to try now.",
                                );
                                widgets::on_click(&offline, &mut app.actions, Action::Refresh);
                            }
                        });
                    });
                },
            );
            // The header above took its height through `scope_builder`.
            match app.section {
                Section::Chats => chat_list::show(app, ui),
                Section::Teams => teams::show(app, ui),
                Section::Calendar => calendar::list(app, ui),
                Section::Calls => calls::list(app, ui),
            }
        });
    let width = response.response.rect.width();
    if (width - app.settings.sidebar_width).abs() > 1.0 && !ui.ctx().input(|i| i.pointer.any_down())
    {
        app.settings.sidebar_width = width;
        app.mark_settings_dirty();
    }
    // The line between the sidebar and the content.
    let rect = response.response.rect;
    ui.painter().vline(
        rect.right(),
        rect.y_range(),
        Stroke::new(1.0, palette.outline),
    );
}

fn main(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    egui::CentralPanel::no_frame()
        .frame(Frame::new().fill(palette.window))
        .show(ui, |ui| match app.section {
            Section::Chats | Section::Teams => match app.open.clone() {
                Some(open) => conversation::show(app, ui, open),
                None => {
                    let header = egui::Rect::from_min_size(
                        ui.max_rect().min,
                        vec2(ui.max_rect().width(), theme::HEADER_HEIGHT),
                    );
                    widgets::window_drag(ui, header, "main-drag");
                    let (title, text) = if app.section == Section::Chats {
                        (
                            "No chat open",
                            "Pick a chat on the left, or press ⌘K to jump to anyone.",
                        )
                    } else {
                        ("No channel open", "Pick a channel on the left.")
                    };
                    widgets::empty_state(ui, &palette, Icon::Chat, title, text);
                }
            },
            Section::Calendar => calendar::detail(app, ui),
            Section::Calls => calls::show(app, ui),
        });
}

fn toasts(app: &mut App, ctx: &egui::Context) {
    if app.toasts.is_empty() {
        return;
    }
    let palette = app.palette;
    let now = ctx.input(|i| i.time);
    let shown_id = egui::Id::new("chaat-toast-times");
    let mut times: std::collections::HashMap<u64, f64> =
        ctx.data(|d| d.get_temp(shown_id)).unwrap_or_default();
    for toast in &app.toasts {
        times.entry(toast.id).or_insert(now);
    }
    // Information fades after a few seconds; errors stay until dismissed.
    let mut expired = Vec::new();
    for toast in &app.toasts {
        if toast.kind == ToastKind::Info {
            let age = now - times[&toast.id];
            if age > 3.5 {
                expired.push(toast.id);
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(3.6 - age));
            }
        }
    }
    for id in expired {
        app.actions.push(Action::DismissToast(id));
    }
    times.retain(|id, _| app.toasts.iter().any(|t| t.id == *id));
    ctx.data_mut(|d| d.insert_temp(shown_id, times));

    egui::Area::new(egui::Id::new("chaat-toasts"))
        .anchor(Align2::CENTER_BOTTOM, vec2(0.0, -96.0))
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            for toast in app.toasts.clone() {
                Frame::new()
                    .fill(palette.surface)
                    .stroke(Stroke::new(
                        1.0,
                        if toast.kind == ToastKind::Error {
                            palette.danger.gamma_multiply(0.6)
                        } else {
                            palette.outline
                        },
                    ))
                    .corner_radius(CornerRadius::same(10))
                    .shadow(egui::Shadow {
                        offset: [0, 4],
                        blur: 16,
                        spread: 0,
                        color: palette.shadow,
                    })
                    .inner_margin(Margin::symmetric(14, 9))
                    .show(ui, |ui| {
                        ui.set_max_width(ui.available_width().min(520.0));
                        ui.horizontal(|ui| {
                            let (icon, tint) = match toast.kind {
                                ToastKind::Error => (Icon::Alert, palette.danger),
                                ToastKind::Info => (Icon::Info, palette.secondary),
                            };
                            theme::icon(ui, icon, 16.0, tint);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(&toast.text).color(palette.text),
                                )
                                .wrap(),
                            );
                            if toast.kind == ToastKind::Error {
                                let close = widgets::icon_button_sized(
                                    ui,
                                    &palette,
                                    Icon::Close,
                                    "Dismiss",
                                    22.0,
                                    14.0,
                                    palette.dim,
                                );
                                widgets::on_click(
                                    &close,
                                    &mut app.actions,
                                    Action::DismissToast(toast.id),
                                );
                            }
                        });
                    });
                ui.add_space(6.0);
            }
        });
}

/// The call-alerts cards at the top right: the plugin's sign-in panel while it
/// waits for one, then the ring toasts, a call the plugin reported with "Place
/// call" and "Open chat". A ring stays until used, dismissed, or about as long
/// as Teams rings (the plugin cannot tell when the call stops).
fn rings(app: &mut App, ctx: &egui::Context) {
    if app.rings.is_empty() && app.plugins.values().all(|state| state.sign_in.is_empty()) {
        return;
    }
    let palette = app.palette;
    for ring in &app.rings {
        let age = ring.arrived.elapsed();
        if age >= ring.shown_for {
            app.actions.push(Action::DismissRing(ring.id.clone()));
        } else {
            ctx.request_repaint_after(ring.shown_for - age);
        }
    }
    let width = (ctx.content_rect().width() - 32.0).clamp(200.0, 340.0);
    let card = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
        Frame::new()
            .fill(palette.surface)
            .stroke(Stroke::new(1.0, palette.accent.gamma_multiply(0.6)))
            .corner_radius(CornerRadius::same(10))
            .shadow(egui::Shadow {
                offset: [0, 4],
                blur: 16,
                spread: 0,
                color: palette.shadow,
            })
            .inner_margin(Margin::same(12))
            .show(ui, |ui| {
                ui.set_width(width - 24.0);
                add(ui);
            });
        ui.add_space(6.0);
    };
    egui::Area::new(egui::Id::new("chaat-rings"))
        .anchor(Align2::RIGHT_TOP, vec2(-16.0, theme::HEADER_HEIGHT + 8.0))
        .order(egui::Order::Foreground)
        .interactable(true)
        .show(ctx, |ui| {
            // One card per (plugin, account).
            let sign_ins: Vec<(crate::model::PluginSignIn, Option<String>, bool)> = app
                .plugins
                .values()
                .flat_map(|state| {
                    let call_alerts = state.offers.contains(&crate::plugin::registry::CALL_ALERTS);
                    state
                        .sign_in
                        .iter()
                        .map(move |s| (s.clone(), state.info.name.clone(), call_alerts))
                })
                .collect();
            for (sign_in, name, call_alerts) in sign_ins {
                let key = (sign_in.plugin.clone(), sign_in.account.clone());
                ui.push_id(key, |ui| {
                    card(ui, &mut |ui| {
                        sign_in_card(
                            ui,
                            &palette,
                            name.as_deref(),
                            call_alerts,
                            &sign_in,
                            &mut app.actions,
                        )
                    });
                });
            }
            for ring in app.rings.clone().iter().rev() {
                card(ui, &mut |ui| {
                    ring_card(ui, &palette, ring, &mut app.actions)
                });
            }
        });
}

/// The call-alerts plugin's sign-in: the code to enter and where. The plugin
/// signs in to Teams on its own; Chaat never shares its own sign-in with it.
fn sign_in_card(
    ui: &mut egui::Ui,
    palette: &crate::theme::Palette,
    plugin: Option<&str>,
    call_alerts: bool,
    sign_in: &crate::model::PluginSignIn,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        theme::icon(ui, Icon::Phone, 20.0, palette.accent);
        ui.label(
            egui::RichText::new(if call_alerts {
                "Sign in for call alerts"
            } else {
                "Sign in for a plugin"
            })
            .font(theme::semibold(15.0))
            .color(palette.text),
        );
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            let close = widgets::icon_button_sized(
                ui,
                palette,
                Icon::Close,
                "Hide",
                22.0,
                14.0,
                palette.dim,
            );
            widgets::on_click(
                &close,
                actions,
                Action::HidePluginSignIn {
                    plugin: sign_in.plugin.clone(),
                    account: sign_in.account.clone(),
                },
            );
        });
    });
    ui.add_space(4.0);
    ui.add(
        egui::Label::new(
            egui::RichText::new(sign_in_text(call_alerts, sign_in.username.as_deref()))
                .size(12.0)
                .color(palette.secondary),
        )
        .wrap(),
    );
    // The plugin's own name, quoted and apart from Chaat's words.
    if let Some(plugin) = plugin {
        ui.add(
            egui::Label::new(
                egui::RichText::new(format!("Plugin: \u{201C}{plugin}\u{201D}"))
                    .size(11.0)
                    .color(palette.dim),
            )
            .truncate(),
        );
    }
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(&sign_in.user_code)
            .font(theme::semibold(24.0))
            .color(palette.text),
    );
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let open = widgets::accent_button(
            ui,
            palette,
            true,
            "Open sign-in page",
            theme::semibold(13.0),
            30.0,
        )
        .on_hover_text(&sign_in.verification_uri);
        widgets::on_click(
            &open,
            actions,
            Action::OpenUrl(sign_in.verification_uri.clone()),
        );
        let copy = widgets::secondary_button(ui, palette, "Copy code");
        widgets::on_click(&copy, actions, Action::CopyText(sign_in.user_code.clone()));
    });
}

fn ring_card(
    ui: &mut egui::Ui,
    palette: &crate::theme::Palette,
    ring: &crate::model::Ring,
    actions: &mut Vec<Action>,
) {
    ui.horizontal(|ui| {
        let icon = if ring.video { Icon::Video } else { Icon::Phone };
        theme::icon(ui, icon, 20.0, palette.accent);
        ui.vertical(|ui| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(&ring.caller)
                        .font(theme::semibold(15.0))
                        .color(palette.text),
                )
                .truncate(),
            );
            let what = match (ring.group, ring.video) {
                (true, _) => "Group call",
                (false, true) => "Incoming video call",
                (false, false) => "Incoming call",
            };
            let what = match &ring.account_name {
                Some(name) => format!("{what} for {name}"),
                None => what.to_owned(),
            };
            ui.add(
                egui::Label::new(
                    egui::RichText::new(what)
                        .size(12.0)
                        .color(palette.secondary),
                )
                .truncate(),
            );
            // Chaat knows no one by this name: it is only what the plugin said.
            if ring.member.is_none() {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new("Name from the plugin")
                            .size(11.0)
                            .color(palette.dim),
                    )
                    .truncate(),
                );
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            let close = widgets::icon_button_sized(
                ui,
                palette,
                Icon::Close,
                "Dismiss",
                22.0,
                14.0,
                palette.dim,
            );
            widgets::on_click(&close, actions, Action::DismissRing(ring.id.clone()));
        });
    });
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        let place =
            widgets::accent_button(ui, palette, true, "Place call", theme::semibold(13.0), 30.0)
                .on_hover_text(if ring.email().is_some() {
                    "Call back in Teams"
                } else {
                    "Open Teams to call back"
                });
        widgets::on_click(&place, actions, Action::RingPlaceCall(ring.id.clone()));
        let open = ui
            .add_enabled_ui(ring.can_open_chat(), |ui| {
                widgets::secondary_button(ui, palette, "Open chat")
            })
            .inner
            .on_disabled_hover_text("Chaat has no chat with this caller yet");
        widgets::on_click(&open, actions, Action::RingOpenChat(ring.id.clone()));
    });
}
fn perf(app: &App, ctx: &egui::Context) {
    let palette = app.palette;
    let max = app.perf.cpu.iter().copied().fold(0.0f32, f32::max) * 1000.0;
    let messages: usize = app.conversations.values().map(|c| c.messages.len()).sum();
    let first = app
        .perf
        .first_frame
        .map_or_else(|| "–".into(), |d| format!("{} ms", d.as_millis()));
    let text = format!(
        "frame {:.2} ms avg · {:.2} ms max · {} frames · first frame {} · {} chats · {} messages loaded",
        app.perf.average_ms(),
        max,
        app.perf.frames,
        first,
        app.chats.len(),
        messages,
    );
    egui::Area::new(egui::Id::new("chaat-perf"))
        .anchor(Align2::RIGHT_BOTTOM, vec2(-12.0, -12.0))
        .order(egui::Order::Tooltip)
        .interactable(false)
        .show(ctx, |ui| {
            Frame::new()
                .fill(palette.surface.gamma_multiply(0.92))
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(8, 4))
                .show(ui, |ui| {
                    ui.label(
                        egui::RichText::new(text)
                            .font(theme::mono(11.0))
                            .color(palette.secondary),
                    );
                });
        });
}

/// Per-window setup: image loaders, icons, fonts, colour emoji.
pub fn install(ctx: &egui::Context, reproducible: bool) {
    egui_extras::install_image_loaders(ctx);
    crate::images::install(ctx);
    fastframe_icons::install::<theme::Icon>(ctx);
    fastframe_titlebar::install(ctx);
    ctx.include_bytes(
        signin::LOGO,
        include_bytes!("../../packaging/icons/chaat-wordmark-light.svg").as_slice(),
    );
    ctx.include_bytes(
        rail::APP_ICON,
        include_bytes!("../../packaging/icons/chaat.svg").as_slice(),
    );
    theme::install_fonts(ctx, reproducible);
    fastframe_emoji::EmojiSetup::default()
        .synchronous(reproducible)
        .install();
    ctx.add_plugin(fastframe_emoji::EmojiPlugin::default());
    if !reproducible {
        let _ = std::thread::Builder::new()
            .name("emoji-font".into())
            .spawn(fastframe_emoji::warm_up);
    }
    ctx.options_mut(|options| options.zoom_with_keyboard = false);
}

/// What the sign-in card says. Fixed wording: the only variable part is the
/// sign-in name of Chaat's own account, never text from the plugin.
fn sign_in_text(call_alerts: bool, username: Option<&str>) -> String {
    let intro = if call_alerts {
        "The call-alerts plugin signs in separately from Chaat."
    } else {
        "The plugin signs in separately from Chaat."
    };
    match username.filter(|name| !name.is_empty()) {
        Some(name) => format!("{intro} Open the page, enter this code, and sign in as {name}:"),
        None => format!("{intro} Open the page and enter this code:"),
    }
}
