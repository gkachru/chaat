//! The setup file group in settings and its dialogs: import, and export
//! (the form, then the passphrase).

use super::widgets::note;
use super::{onboarding, widgets};
use crate::app::{App, SetupStage};
use crate::backend::AuthStatus;
use crate::model::Action;
use crate::theme;

/// The "Setup file" group on Settings › Advanced. Hidden in demo mode.
/// Both flows open in a dialog over the page.
pub fn group(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    if app.demo {
        return;
    }
    let palette = app.palette;
    super::widgets::group(ui, &palette, "Setup file");
    let busy = app.setup.stage != SetupStage::Idle || app.export.open;
    ui.horizontal(|ui| {
        if ui
            .add_enabled(!busy, egui::Button::new("Import a setup file…"))
            .clicked()
        {
            actions.push(Action::ChooseSetupFile);
        }
        if ui
            .add_enabled(!busy, egui::Button::new("Create a setup file…"))
            .clicked()
        {
            actions.push(Action::OpenExportForm(true));
        }
    });
    note(
        ui,
        &palette,
        "A setup file carries the app registration and, optionally, the push relay, so others can start without typing them.",
    );
}

/// The "Create a setup file" dialog: the form, then the passphrase.
pub fn export_overlay(app: &mut App, ctx: &egui::Context) {
    if !app.export.open && app.export.passphrase.is_none() {
        return;
    }
    let palette = app.palette;
    let mut actions = Vec::new();
    let revealing = app.export.passphrase.is_some();
    let look = widgets::ModalLook {
        margin: 20,
        shadow: false,
        top: None,
    };
    let response = widgets::modal(palette, ctx, "chaat-setup-export", 440.0, look, |ui| {
        if revealing {
            reveal(app, ui, &mut actions);
        } else {
            export_form(app, ui, &mut actions);
        }
    });
    // Escape or a click outside cancels the form. The passphrase is shown
    // only once, so only Done closes it.
    if response.should_close() && !revealing && !app.export.busy {
        actions.push(Action::OpenExportForm(false));
    }
    app.actions.extend(actions);
}

fn heading(ui: &mut egui::Ui, palette: &theme::Palette, title: &str, subtitle: &str) {
    ui.label(
        egui::RichText::new(title)
            .font(theme::semibold(18.0))
            .color(palette.text),
    );
    ui.add(
        egui::Label::new(
            egui::RichText::new(subtitle)
                .font(theme::regular(13.0))
                .color(palette.secondary),
        )
        .wrap(),
    );
}

fn export_form(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let has_relay = app.settings.relay_url.is_some() && app.settings.relay_key_set;
    let busy = app.export.busy;
    heading(
        ui,
        &palette,
        "Create a setup file",
        "People who import it get these settings. The file is encrypted with a passphrase you send separately.",
    );

    super::widgets::group(ui, &palette, "Included");
    ui.add_space(4.0);
    widgets::field_label(ui, &palette, "App registration");
    ui.add(
        egui::Label::new(
            egui::RichText::new(format!(
                "{} · {}",
                app.settings.client_id, app.settings.tenant
            ))
            .font(theme::regular(13.0))
            .color(palette.text),
        )
        .wrap(),
    );
    if has_relay {
        ui.add_space(4.0);
        ui.add_enabled(
            !busy,
            egui::Checkbox::new(
                &mut app.export.include_relay,
                "Include the push relay (address and access key)",
            ),
        );
    }

    super::widgets::group(ui, &palette, "Shown to people who import it");
    ui.add_space(4.0);
    widgets::field_label(ui, &palette, "Organization name");
    ui.add_enabled(
        !busy,
        widgets::text_field(
            &mut app.export.org_label,
            "For example, Contoso",
            theme::regular(14.0),
        ),
    );
    let label = app.export.org_label.trim().to_owned();
    note(
        ui,
        &palette,
        if label.is_empty() {
            "Required. They see it as \"Set up for …\". It does not change who can sign in."
                .to_owned()
        } else {
            format!("They see \"Set up for {label}\".")
        },
    );

    super::widgets::group(ui, &palette, "Expiry");
    ui.add_space(4.0);
    widgets::field_label(ui, &palette, "Stop accepting after (optional)");
    ui.add_enabled(
        !busy,
        widgets::text_field(
            &mut app.export.import_by,
            "YYYY-MM-DD",
            theme::regular(14.0),
        ),
    );

    if let Some(error) = &app.export.error {
        ui.add_space(6.0);
        ui.add(
            egui::Label::new(
                egui::RichText::new(error)
                    .font(theme::regular(13.0))
                    .color(palette.danger),
            )
            .wrap(),
        );
    }
    ui.add_space(14.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let ready = !busy && !label.is_empty();
        if widgets::accent_button(
            ui,
            &palette,
            ready,
            "  Create file…  ",
            theme::semibold(14.0),
            34.0,
        )
        .clicked()
        {
            actions.push(Action::ExportSetup);
        }
        if busy {
            widgets::loading(ui, palette.secondary, 14.0);
        } else if widgets::secondary_button(ui, &palette, "  Cancel  ").clicked() {
            actions.push(Action::OpenExportForm(false));
        }
    });
}

fn reveal(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let Some(passphrase) = app.export.passphrase.clone() else {
        return;
    };
    heading(
        ui,
        &palette,
        "Setup file created",
        "Send the passphrase separately from the file, for example by phone or another chat. Chaat shows it only once.",
    );
    ui.add_space(12.0);
    ui.add(
        egui::Label::new(
            egui::RichText::new(&passphrase)
                .font(theme::mono(18.0))
                .color(palette.text),
        )
        .wrap()
        .selectable(true),
    );
    ui.add_space(14.0);
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        if widgets::primary_button(ui, &palette, "  Done  ").clicked() {
            actions.push(Action::DismissSetupPassphrase);
        }
        if widgets::secondary_button(ui, &palette, "  Copy  ").clicked() {
            actions.push(Action::CopyText(passphrase));
        }
    });
}

/// Whether applying `summary` changes the app registration while accounts
/// are signed in, which signs them all in again.
pub(crate) fn resigns(app: &App, summary: &crate::setup_file::SetupSummary) -> bool {
    let signed_in = app
        .accounts
        .iter()
        .any(|account| matches!(account.auth, AuthStatus::SignedIn { .. }));
    signed_in
        && (summary.client_id != app.settings.client_id || summary.tenant != app.settings.tenant)
}

pub fn import_overlay(app: &mut App, ctx: &egui::Context) {
    let stage = app.setup.stage.clone();
    if stage == SetupStage::Idle {
        return;
    }
    let palette = app.palette;
    let warn = matches!(&stage, SetupStage::Review(summary) if resigns(app, summary));
    let look = widgets::ModalLook {
        margin: 20,
        shadow: false,
        top: None,
    };
    let response = widgets::modal(
        palette,
        ctx,
        "chaat-setup-import",
        420.0,
        look,
        |ui| match &stage {
            SetupStage::Idle => {}
            SetupStage::Passphrase(_) | SetupStage::Opening(_) => {
                onboarding::passphrase(app, ui);
            }
            SetupStage::Review(_) => onboarding::review(app, ui, warn, false),
            SetupStage::Applying => {
                ui.horizontal(|ui| {
                    widgets::loading(ui, palette.secondary, 14.0);
                    ui.label(egui::RichText::new("Applying…").color(palette.text));
                });
            }
            SetupStage::Failed(message) => {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(message)
                            .font(theme::regular(14.0))
                            .color(palette.danger),
                    )
                    .wrap(),
                );
                ui.add_space(8.0);
                if ui.button("Close").clicked() {
                    app.actions.push(Action::CancelSetup);
                }
            }
        },
    );
    let working = matches!(stage, SetupStage::Opening(_) | SetupStage::Applying);
    if response.should_close() && !working {
        app.actions.push(Action::CancelSetup);
    }
}
