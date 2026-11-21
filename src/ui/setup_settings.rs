//! The setup file group in settings: import, export form, passphrase reveal.

use egui::{CornerRadius, Frame, Margin, Stroke};

use super::settings::note;
use super::{onboarding, widgets};
use crate::app::{App, SetupStage};
use crate::backend::AuthStatus;
use crate::model::Action;
use crate::theme;

/// The "Setup file" group on Settings › Advanced. Hidden in demo mode.
pub fn group(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    if app.demo {
        return;
    }
    let palette = app.palette;
    group_heading(ui, &palette);
    let busy = app.setup.stage != SetupStage::Idle;
    if ui
        .add_enabled(!busy, egui::Button::new("Import a setup file…"))
        .clicked()
    {
        actions.push(Action::ChooseSetupFile);
    }
    note(
        ui,
        &palette,
        "Fills in the app registration and push relay from a file your administrator made.",
    );
    ui.add_space(6.0);
    if app.export.passphrase.is_some() {
        reveal(app, ui, actions);
    } else if app.export.open {
        export_form(app, ui, actions);
    } else if ui.button("Create a setup file…").clicked() {
        actions.push(Action::OpenExportForm(true));
    }
}

fn group_heading(ui: &mut egui::Ui, palette: &theme::Palette) {
    super::settings::group(ui, palette, "Setup file");
}

fn export_form(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let has_tenant =
        crate::settings::is_guid(&app.settings.tenant) || app.export_tenant().is_some();
    let has_relay = app.settings.relay_url.is_some() && app.settings.relay_key_set;
    let busy = app.export.busy;
    ui.label(egui::RichText::new("Organization name").color(palette.secondary));
    ui.add_enabled(
        !busy,
        widgets::text_field(
            &mut app.export.org_label,
            "Organization name",
            theme::regular(14.0),
        ),
    );
    if has_tenant {
        ui.add_enabled(
            !busy,
            egui::Checkbox::new(
                &mut app.export.limit_tenant,
                "Only allow accounts from this tenant",
            ),
        );
    }
    if has_relay {
        ui.add_enabled(
            !busy,
            egui::Checkbox::new(
                &mut app.export.include_relay,
                "Include the push relay (address and access key)",
            ),
        );
    }
    ui.label(egui::RichText::new("Stop accepting after (optional)").color(palette.secondary));
    ui.add_enabled(
        !busy,
        widgets::text_field(
            &mut app.export.import_by,
            "YYYY-MM-DD",
            theme::regular(14.0),
        ),
    );
    if let Some(error) = &app.export.error {
        ui.label(
            egui::RichText::new(error)
                .font(theme::regular(13.0))
                .color(palette.danger),
        );
    }
    let named = !app.export.org_label.trim().is_empty();
    if !named {
        note(
            ui,
            &palette,
            "Enter an organization name to create the file. People see it when they import.",
        );
    }
    ui.horizontal(|ui| {
        let ready = !busy && named;
        if ui
            .add_enabled(ready, egui::Button::new("Create file…"))
            .clicked()
        {
            actions.push(Action::ExportSetup);
        }
        if busy {
            widgets::loading(ui, palette.secondary, 14.0);
        } else if ui.button("Cancel").clicked() {
            actions.push(Action::OpenExportForm(false));
        }
    });
}

fn reveal(app: &mut App, ui: &mut egui::Ui, actions: &mut Vec<Action>) {
    let palette = app.palette;
    let Some(passphrase) = app.export.passphrase.clone() else {
        return;
    };
    ui.label(egui::RichText::new("Setup file created. Passphrase:").color(palette.text));
    ui.add(
        egui::Label::new(
            egui::RichText::new(&passphrase)
                .font(theme::mono(18.0))
                .color(palette.text),
        )
        .wrap()
        .selectable(true),
    );
    ui.horizontal(|ui| {
        if ui.button("Copy").clicked() {
            actions.push(Action::CopyText(passphrase));
        }
        if ui.button("Done").clicked() {
            actions.push(Action::DismissSetupPassphrase);
        }
    });
    note(
        ui,
        &palette,
        "Send the passphrase separately from the file, for example by phone or another chat.",
    );
}

/// Whether applying `summary` changes the app registration while accounts
/// are signed in, which signs them all in again.
fn resigns(app: &App, summary: &crate::setup_file::SetupSummary) -> bool {
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
    let response = egui::Modal::new(egui::Id::new("chaat-setup-import"))
        .backdrop_color(palette.overlay)
        .frame(
            Frame::new()
                .fill(palette.surface)
                .stroke(Stroke::new(1.0, palette.outline))
                .corner_radius(CornerRadius::same(14))
                .inner_margin(Margin::same(20)),
        )
        .show(ctx, |ui| {
            ui.set_width(420.0);
            match &stage {
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
            }
        });
    let working = matches!(stage, SetupStage::Opening(_) | SetupStage::Applying);
    if response.should_close() && !working {
        app.actions.push(Action::CancelSetup);
    }
}
