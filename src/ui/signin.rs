//! The welcome screen: connect an app registration and sign in.

use egui::{Align2, CornerRadius, Frame, Margin, Stroke, vec2};

use super::widgets;
use crate::app::App;
use crate::app::SetupStage;
use crate::backend::AuthStatus;
use crate::model::Action;
use crate::theme;
use crate::theme::Icon;

pub const LOGO: &str = "bytes://chaat-logo.svg";
pub const DOCS: &str = "https://github.com/gkachru/chaat/blob/main/docs/azure-app-registration.md";

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    egui::CentralPanel::no_frame()
        .frame(Frame::new().fill(palette.panel))
        .show(ui, |ui| {
            let header = egui::Rect::from_min_size(ui.max_rect().min, vec2(ui.max_rect().width(), theme::HEADER_HEIGHT));
            widgets::window_drag(ui, header, "signin-drag");
            egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space((ui.available_height() * 0.10).max(48.0));
                    ui.add(egui::Image::new(LOGO).fit_to_exact_size(vec2(160.0, 121.0)).tint(palette.text).alt_text("Chaat"));
                    ui.add_space(10.0);
                    ui.label(
                        egui::RichText::new("A fast, native client for Microsoft Teams chats, channels and meetings.")
                            .font(theme::regular(14.0))
                            .color(palette.secondary),
                    );
                    ui.add_space(22.0);
                    Frame::new()
                        .fill(palette.surface)
                        .stroke(Stroke::new(1.0, palette.outline))
                        .corner_radius(CornerRadius::same(14))
                        .inner_margin(Margin::same(24))
                        .shadow(egui::Shadow {
                            offset: [0, 6],
                            blur: 24,
                            spread: 0,
                            color: palette.shadow,
                        })
                        .show(ui, |ui| {
                            let wide = if railed(app) {
                                (ui.available_width() - 48.0).clamp(280.0, 592.0)
                            } else {
                                420.0
                            };
                            ui.set_width(wide);
                            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                                card(app, ui);
                            });
                        });
                    ui.add_space(18.0);
                    // Adding a second account: the way back.
                    if let Some(previous) = app.adding_from().map(|account| account.label())
                        && widgets::secondary_button(ui, &palette, &format!("  Back to {previous}  ")).clicked()
                    {
                        app.actions.push(Action::CancelAddAccount);
                    }
                    ui.add_space(24.0);
                    ui.label(
                        egui::RichText::new("Chaat is an independent project based on the open-source Wren app, not affiliated with Microsoft. It talks only to Microsoft's official APIs and keeps your data on this computer.")
                            .font(theme::regular(12.0))
                            .color(palette.dim),
                    );
                    ui.add_space(24.0);
                });
            });
        });
}

/// First run: nothing has ever signed in, so the setup steps frame the card.
fn railed(app: &App) -> bool {
    if app.onboarding_forced {
        return true;
    }
    match app.auth {
        AuthStatus::NotConfigured => true,
        AuthStatus::SignedOut
        | AuthStatus::Restoring
        | AuthStatus::Finishing
        | AuthStatus::WaitingForBrowser { .. }
        | AuthStatus::DeviceCode { .. }
        | AuthStatus::Failed(_) => {
            app.adding_from().is_none()
                && app.settings.accounts.is_empty()
                && !app
                    .accounts
                    .iter()
                    .any(|account| matches!(account.auth, AuthStatus::SignedIn { .. }))
        }
        _ => false,
    }
}

fn card(app: &mut App, ui: &mut egui::Ui) {
    // Step one: no registration yet, forced by CHAAT_ONBOARDING, or another
    // setup file chosen from the sign-in step.
    if matches!(app.auth, AuthStatus::NotConfigured)
        || app.onboarding_forced
        || (railed(app) && app.setup.stage != SetupStage::Idle)
    {
        return onboarding(app, ui);
    }
    if railed(app) {
        return stepped(app, ui, 1, state_card);
    }
    state_card(app, ui);
}

fn state_card(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    ui.spacing_mut().item_spacing.y = 8.0;
    match app.auth.clone() {
        AuthStatus::NotConfigured => {}
        AuthStatus::Restoring | AuthStatus::Finishing => {
            ui.vertical_centered(|ui| {
                widgets::loading(ui, palette.accent, 22.0);
                ui.label(egui::RichText::new("Signing in…").color(palette.secondary));
            });
        }
        AuthStatus::WaitingForBrowser { url } => {
            ui.label(
                egui::RichText::new("Continue in your browser")
                    .font(theme::semibold(18.0))
                    .color(palette.text),
            );
            ui.label(
                egui::RichText::new("Sign in with your work or school account in the browser window Chaat opened. You'll come back here automatically.")
                    .color(palette.secondary),
            );
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if widgets::secondary_button(ui, &palette, "Open the page again").clicked() {
                    app.actions.push(Action::OpenUrl(url.clone()));
                }
                if widgets::secondary_button(ui, &palette, "Copy link").clicked() {
                    app.actions.push(Action::CopyText(url.clone()));
                }
                if widgets::secondary_button(ui, &palette, "Cancel").clicked() {
                    app.actions.push(Action::CancelSignIn);
                }
            });
        }
        AuthStatus::DeviceCode { code, url } => {
            ui.label(
                egui::RichText::new("Sign in with a code")
                    .font(theme::semibold(18.0))
                    .color(palette.text),
            );
            ui.label(
                egui::RichText::new(format!("On any device, open {url} and enter:"))
                    .color(palette.secondary),
            );
            ui.add_space(4.0);
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new(&code)
                        .font(theme::mono(28.0))
                        .color(palette.accent),
                );
            });
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &palette, "Open the page").clicked() {
                    app.actions.push(Action::OpenUrl(url.clone()));
                }
                if widgets::secondary_button(ui, &palette, "Copy code").clicked() {
                    app.actions.push(Action::CopyText(code.clone()));
                }
                if widgets::secondary_button(ui, &palette, "Cancel").clicked() {
                    app.actions.push(Action::CancelSignIn);
                }
            });
        }
        status => {
            let (title, text) = match &status {
                AuthStatus::Expired { username, reason } => (
                    "Sign in again".to_owned(),
                    if username.is_empty() { reason.clone() } else { format!("{reason} ({username})") },
                ),
                AuthStatus::Failed(reason) => ("Sign-in did not finish".to_owned(), reason.clone()),
                _ => (
                    "Sign in".to_owned(),
                    "Use your work or school Microsoft 365 account. Microsoft's sign-in page opens in your browser; Chaat never sees your password.".to_owned(),
                ),
            };
            ui.label(
                egui::RichText::new(title)
                    .font(theme::semibold(18.0))
                    .color(palette.text),
            );
            ui.label(
                egui::RichText::new(text).color(if matches!(status, AuthStatus::Failed(_)) {
                    palette.danger
                } else {
                    palette.secondary
                }),
            );
            if let Some(org) = app.settings.org_label.clone().filter(|org| !org.is_empty()) {
                ui.label(
                    egui::RichText::new(format!("Set up for {org}"))
                        .font(theme::medium(13.0))
                        .color(palette.text),
                );
            }
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                if widgets::primary_button(ui, &palette, "  Sign in with Microsoft  ").clicked() {
                    app.actions.push(Action::SignIn);
                }
                if widgets::secondary_button(ui, &palette, "Use a code instead")
                    .on_hover_text("Device code sign-in, for when the browser can't reach Chaat. Needs 'Allow public client flows' on the app registration.")
                    .clicked()
                {
                    app.actions.push(Action::SignInWithCode);
                }
            });
            // First run: the way back to step one, if the wrong file was used.
            if railed(app) && !matches!(status, AuthStatus::Expired { .. }) {
                ui.add_space(4.0);
                if ui.link("Use a different setup file").clicked() {
                    app.actions.push(Action::ChooseSetupFile);
                }
            }
            ui.add_space(8.0);
            ui.separator();
            egui::CollapsingHeader::new(
                egui::RichText::new("App registration")
                    .font(theme::medium(13.0))
                    .color(palette.secondary),
            )
            .id_salt("registration")
            .default_open(false)
            .show(ui, |ui| registration(app, ui, false))
            .header_response
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        }
    }
}

const STEPS: [&str; 3] = [
    "Connect to your organization",
    "Sign in",
    "Real-time updates",
];
const RAIL_WIDTH: f32 = 170.0;

/// First run, before a registration exists: the connect step.
fn onboarding(app: &mut App, ui: &mut egui::Ui) {
    stepped(app, ui, 0, connect_step);
}

/// A step rail (or "Step N of 3") beside `body`, with `step` current.
fn stepped(
    app: &mut App,
    ui: &mut egui::Ui,
    step: usize,
    body: impl FnOnce(&mut App, &mut egui::Ui),
) {
    let narrow = ui.ctx().content_rect().width() < 720.0;
    let relay = match &app.setup.stage {
        SetupStage::Review(summary) if step == 0 => Some(summary.relay_url.is_some()),
        _ if step > 0 => Some(app.settings.relay_url.is_some() && app.settings.relay_key_set),
        _ => None,
    };
    if narrow {
        // The step's own heading follows, so only the position goes here.
        let palette = app.palette;
        ui.label(
            egui::RichText::new(format!("Step {} of 3", step + 1))
                .font(theme::medium(12.0))
                .color(palette.dim),
        );
        ui.add_space(4.0);
        body(app, ui);
    } else {
        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(
                vec2(RAIL_WIDTH, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_width(RAIL_WIDTH);
                    rail(app, ui, step, relay);
                },
            );
            ui.add_space(12.0);
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| body(app, ui),
            );
        });
    }
}

/// The numbered steps; finished ones show a check, the current one the accent.
fn rail(app: &App, ui: &mut egui::Ui, step: usize, relay: Option<bool>) {
    let palette = app.palette;
    ui.spacing_mut().item_spacing.y = 14.0;
    for (index, name) in STEPS.iter().enumerate() {
        let current = index == step;
        let done = index < step || (index == 2 && relay == Some(true));
        let colour = if done || current {
            palette.accent
        } else {
            palette.dim
        };
        ui.horizontal_top(|ui| {
            let state = if done {
                "done"
            } else if current {
                "current step"
            } else {
                "not started"
            };
            // Screen readers hear the number and state the circle paints.
            let spoken = format!("Step {}, {state}", index + 1);
            if done {
                theme::icon(ui, Icon::Check, 20.0, palette.accent).widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &spoken)
                });
            } else {
                let (rect, response) =
                    ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::hover());
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Label, true, &spoken)
                });
                let fill = if current {
                    palette.accent
                } else {
                    palette.surface_hover
                };
                ui.painter().circle_filled(rect.center(), 10.0, fill);
                ui.painter().text(
                    rect.center(),
                    Align2::CENTER_CENTER,
                    (index + 1).to_string(),
                    theme::semibold(11.0),
                    if current {
                        palette.on_accent
                    } else {
                        palette.dim
                    },
                );
            }
            ui.vertical(|ui| {
                ui.set_max_width(RAIL_WIDTH - 30.0);
                ui.label(
                    egui::RichText::new(*name)
                        .font(if current {
                            theme::semibold(13.0)
                        } else {
                            theme::medium(13.0)
                        })
                        .color(if current { palette.text } else { colour }),
                );
                // Explained once the person is past step one, not before
                // they know what it is.
                if index == 2 && relay != Some(true) && step >= 1 {
                    ui.label(
                        egui::RichText::new("Optional. Set up later in Settings › Push updates.")
                            .font(theme::regular(11.0))
                            .color(palette.dim),
                    );
                }
            });
        });
    }
}

/// Step one: a setup file (drop, passphrase, review) or the manual form.
fn connect_step(app: &mut App, ui: &mut egui::Ui) {
    use super::onboarding;
    let palette = app.palette;
    ui.spacing_mut().item_spacing.y = 8.0;
    let stage = app.setup.stage.clone();
    // The review has its own heading ("Set up for …").
    if !matches!(stage, SetupStage::Review(_)) {
        ui.label(
            egui::RichText::new(STEPS[0])
                .font(theme::semibold(18.0))
                .color(palette.text),
        );
    }
    match stage {
        SetupStage::Idle | SetupStage::Failed(_) => {
            let manual = app.setup.manual;
            ui.label(
                egui::RichText::new(if manual {
                    "Enter the details of your organization's app registration."
                } else {
                    "Use the setup file from your administrator, or enter the details yourself."
                })
                .color(palette.secondary),
            );
            ui.add_space(4.0);
            // One way at a time: the file, or the form instead of it.
            if manual {
                registration(app, ui, false);
            } else {
                onboarding::drop_zone(app, ui);
            }
            ui.add_space(2.0);
            let label = if manual {
                "Use a setup file instead"
            } else {
                "Enter details manually"
            };
            if ui.link(label).clicked() {
                app.actions.push(Action::ShowSetupManual(!manual));
            }
        }
        SetupStage::Passphrase(_) | SetupStage::Opening(_) => {
            onboarding::passphrase(app, ui);
        }
        SetupStage::Review(summary) => {
            // Forced over a signed-in Chaat: say what applying costs, and
            // only promise sign-in when nobody is signed in yet.
            let warn = super::setup_settings::resigns(app, &summary);
            let first_run = app.settings.accounts.is_empty();
            onboarding::review(app, ui, warn, first_run);
        }
        SetupStage::Applying => onboarding::applying(app, ui),
    }
    if app.onboarding_forced {
        forced_note(app, ui);
    } else if !matches!(app.auth, AuthStatus::NotConfigured)
        && matches!(app.setup.stage, SetupStage::Failed(_))
        && ui.link("Back to sign in").clicked()
    {
        // Here from the sign-in step: the way back without a new file.
        app.actions.push(Action::CancelSetup);
    }
}

/// Why the welcome screen shows on a configured Chaat, and the way out.
fn forced_note(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    ui.add_space(6.0);
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        ui.label(
            egui::RichText::new(format!(
                "Shown because {}=1 is set. A setup file overrides only the settings it carries.",
                crate::app::ONBOARDING_ENV
            ))
            .font(theme::regular(12.0))
            .color(palette.dim),
        );
        let working = matches!(
            app.setup.stage,
            SetupStage::Opening(_) | SetupStage::Applying
        );
        if !matches!(app.auth, AuthStatus::NotConfigured)
            && !working
            && ui.link("Skip to Chaat").clicked()
        {
            app.actions.push(Action::SkipOnboarding);
        }
    });
}

/// The application (client) ID and tenant Chaat signs in with.
fn registration(app: &mut App, ui: &mut egui::Ui, first_run: bool) {
    let palette = app.palette;
    if first_run {
        ui.label(
            egui::RichText::new("Connect Chaat to Microsoft 365")
                .font(theme::semibold(18.0))
                .color(palette.text),
        );
        ui.label(
            egui::RichText::new("Chaat signs in through an Entra ID app registration that you or your administrator create. It takes about five minutes and needs no secrets.")
                .color(palette.secondary),
        );
        ui.add_space(4.0);
    }
    ui.label(
        egui::RichText::new("Application (client) ID")
            .font(theme::medium(12.0))
            .color(palette.secondary),
    );
    ui.add(widgets::text_field(
        &mut app.form_client_id,
        "00000000-0000-0000-0000-000000000000",
        theme::mono(13.0),
    ));
    ui.label(
        egui::RichText::new("Tenant")
            .font(theme::medium(12.0))
            .color(palette.secondary),
    );
    ui.label(
        egui::RichText::new(
            "Your tenant ID or domain. Use 'organizations' only for a multi-tenant registration.",
        )
        .font(theme::regular(11.0))
        .color(palette.dim),
    );
    ui.add(widgets::text_field(
        &mut app.form_tenant,
        "Your tenant ID or domain, e.g. contoso.onmicrosoft.com",
        theme::regular(13.0),
    ));
    ui.add_space(4.0);
    let valid = crate::settings::is_guid(app.form_client_id.trim());
    ui.horizontal(|ui| {
        let save = widgets::with_fills(ui, palette.accent, palette.accent_hover, |ui| {
            ui.add_enabled(
                valid,
                egui::Button::new(
                    egui::RichText::new("Save")
                        .color(palette.on_accent)
                        .font(theme::semibold(13.0)),
                )
                .corner_radius(CornerRadius::same(8))
                .min_size(vec2(72.0, 32.0)),
            )
        });
        if save.clicked() {
            app.actions.push(Action::SaveAppRegistration {
                client_id: app.form_client_id.clone(),
                tenant: app.form_tenant.clone(),
            });
        }
        if ui.link("How to register Chaat").clicked() {
            app.actions.push(Action::OpenUrl(DOCS.into()));
        }
    });
    if !valid {
        ui.label(
            egui::RichText::new(if app.form_client_id.trim().is_empty() {
                "Enter the application (client) ID to continue."
            } else {
                "The application (client) ID is a GUID like 00000000-0000-0000-0000-000000000000."
            })
            .font(theme::regular(12.0))
            .color(palette.dim),
        );
    }
    ui.label(
        egui::RichText::new("Or start Chaat with CHAAT_CLIENT_ID and CHAAT_TENANT set.")
            .font(theme::regular(12.0))
            .color(palette.dim),
    );
}
