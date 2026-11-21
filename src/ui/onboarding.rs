//! Onboarding views for the setup file: the drop zone, the passphrase entry
//! and the review. They read `app.setup` and push `Action`s.

use egui::{CornerRadius, Sense, Stroke, StrokeKind, vec2};

use crate::app::{App, SetupStage};
use crate::model::Action;
use crate::theme::{self, Icon};
use crate::ui::widgets;

/// What is being dragged over the window.
#[derive(PartialEq)]
enum Hover {
    None,
    Setup,
    Other,
}

fn hover(ctx: &egui::Context) -> Hover {
    ctx.input(|i| {
        let files = &i.raw.hovered_files;
        if files.is_empty() {
            return Hover::None;
        }
        let ok = files.iter().all(|f| {
            f.path
                .as_ref()
                .is_some_and(|p| crate::app::is_setup_file(p))
        });
        if ok { Hover::Setup } else { Hover::Other }
    })
}

/// The drop zone and file chooser.
pub fn drop_zone(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let hover = hover(ui.ctx());
    let failed = match &app.setup.stage {
        SetupStage::Failed(message) => Some(message.clone()),
        _ => None,
    };
    let width = ui.available_width();
    // The icon, two lines and the button take about 128 points; this leaves
    // about 20 above and below.
    let (rect, _) = ui.allocate_exact_size(vec2(width, 170.0), Sense::hover());
    let (fill, stroke) = match hover {
        Hover::Setup => (palette.surface_hover, Stroke::new(2.0, palette.accent)),
        Hover::Other => (palette.surface, Stroke::new(2.0, palette.danger)),
        Hover::None => (
            palette.panel,
            Stroke::new(1.0, palette.accent.gamma_multiply(0.55)),
        ),
    };
    ui.painter().rect_filled(rect, CornerRadius::same(12), fill);
    ui.painter()
        .rect_stroke(rect, CornerRadius::same(12), stroke, StrokeKind::Inside);
    let text = match hover {
        Hover::None => "Drop your setup file here",
        Hover::Setup => "Release to import",
        Hover::Other => "Only a .chaatsetup file can be imported here",
    };
    let mut inner = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect.shrink(16.0))
            .layout(egui::Layout::top_down(egui::Align::Center)),
    );
    inner.add_space(4.0);
    inner.add(Icon::File.image(palette.accent, 24.0));
    inner.add_space(6.0);
    inner.label(
        egui::RichText::new(text)
            .font(theme::medium(14.0))
            .color(palette.text),
    );
    if hover == Hover::None {
        inner.label(
            egui::RichText::new("A .chaatsetup file from your administrator")
                .font(theme::regular(12.0))
                .color(palette.dim),
        );
        inner.add_space(8.0);
        if widgets::secondary_button(&mut inner, &palette, "Choose file").clicked() {
            app.actions.push(Action::ChooseSetupFile);
        }
    }
    if let Some(message) = failed {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new(message)
                .font(theme::regular(13.0))
                .color(palette.danger),
        );
    }
}

/// The passphrase entry.
pub fn passphrase(app: &mut App, ui: &mut egui::Ui) {
    let palette = app.palette;
    let (path, opening) = match &app.setup.stage {
        SetupStage::Passphrase(path) => (path.clone(), false),
        SetupStage::Opening(path) => (path.clone(), true),
        _ => return,
    };
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    ui.horizontal(|ui| {
        ui.add(Icon::File.image(palette.secondary, 18.0));
        widgets::line(ui, &name, theme::medium(14.0), palette.text);
    });
    ui.add_space(6.0);
    ui.label(
        egui::RichText::new(
            "Enter the six-word passphrase. It was sent to you separately from the file.",
        )
        .font(theme::regular(13.0))
        .color(palette.secondary),
    );
    ui.add_space(4.0);
    let mut submit = false;
    // Whether the words are shown: this view's own scratch state.
    let shown_id = ui.id().with("chaat-setup-passphrase-shown");
    let mut shown = ui.data(|d| d.get_temp::<bool>(shown_id).unwrap_or(false));
    ui.add_enabled_ui(!opening, |ui| {
        ui.horizontal(|ui| {
            let toggle = if shown { "Hide" } else { "Show" };
            let toggle_width = widgets::pill_width(ui, toggle) + 8.0;
            let field = widgets::text_field(
                &mut app.setup.passphrase,
                "word-word-word-word-word-word",
                theme::regular(14.0),
            )
            .password(!shown)
            .desired_width(ui.available_width() - toggle_width - 8.0);
            let response = ui.add(field);
            if !response.has_focus() && ui.memory(|m| m.focused().is_none()) {
                response.request_focus();
            }
            if response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                submit = true;
            }
            if widgets::text_button(ui, &palette, toggle, theme::medium(13.0))
                .on_hover_text(if shown {
                    "Hide the passphrase"
                } else {
                    "Show the passphrase to check for typing mistakes"
                })
                .clicked()
            {
                shown = !shown;
            }
        });
    });
    ui.data_mut(|d| d.insert_temp(shown_id, shown));
    if !opening && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.actions.push(Action::CancelSetup);
    }
    if let Some(error) = app.setup.error.clone() {
        ui.add_space(6.0);
        ui.label(
            egui::RichText::new(error)
                .font(theme::regular(13.0))
                .color(palette.danger),
        );
    }
    ui.add_space(12.0);
    if opening {
        ui.horizontal(|ui| {
            widgets::loading(ui, palette.accent, 18.0);
            ui.label(
                egui::RichText::new("Unlocking…")
                    .font(theme::regular(13.0))
                    .color(palette.secondary),
            );
        });
    } else {
        ui.horizontal(|ui| {
            let open = widgets::accent_button(
                ui,
                &palette,
                !app.setup.passphrase.is_empty(),
                "  Open  ",
                theme::semibold(14.0),
                34.0,
            );
            if open.clicked() {
                submit = true;
            }
            if widgets::secondary_button(ui, &palette, "Choose another file").clicked() {
                app.actions.push(Action::ChooseSetupFile);
            }
            if widgets::secondary_button(ui, &palette, "Cancel").clicked() {
                app.actions.push(Action::CancelSetup);
            }
        });
    }
    if submit && !app.setup.passphrase.is_empty() {
        app.actions.push(Action::OpenSetupFile);
    }
}

/// Applying the file, on the welcome screen.
pub fn applying(app: &App, ui: &mut egui::Ui) {
    ui.horizontal(|ui| {
        widgets::loading(ui, app.palette.accent, 18.0);
        ui.label(
            egui::RichText::new("Applying…")
                .font(theme::regular(13.0))
                .color(app.palette.secondary),
        );
    });
}

/// The review of what the file sets. `first_run` is the welcome screen,
/// where applying goes straight on to sign-in.
pub fn review(app: &mut App, ui: &mut egui::Ui, warn_resign: bool, first_run: bool) {
    let palette = app.palette;
    let SetupStage::Review(summary) = &app.setup.stage else {
        return;
    };
    let summary = summary.clone();
    ui.label(
        egui::RichText::new(format!("Set up for {}", summary.org_label))
            .font(theme::semibold(18.0))
            .color(palette.text),
    );
    ui.label(
        egui::RichText::new("Check what this file sets before you use it.")
            .font(theme::regular(13.0))
            .color(palette.secondary),
    );
    ui.add_space(8.0);
    let row = |ui: &mut egui::Ui, label: &str, value: &str| {
        ui.horizontal_top(|ui| {
            ui.add_sized(
                vec2(REVIEW_LABEL_WIDTH, 18.0),
                egui::Label::new(
                    egui::RichText::new(label)
                        .font(theme::regular(13.0))
                        .color(palette.secondary),
                )
                .halign(egui::Align::Min),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(value)
                        .font(theme::regular(13.0))
                        .color(palette.text),
                )
                .wrap(),
            );
        });
    };
    row(ui, "Who can sign in", &who_can_sign_in(&summary));
    let realtime = match (&summary.relay_url, summary.relay_key_included) {
        (Some(url), true) => format!("Included ({})", relay_host(url)),
        (Some(url), false) => format!("Relay {} without an access key", relay_host(url)),
        (None, _) => "Not included".to_owned(),
    };
    row(ui, "Real-time updates", &realtime);
    if let Some(date) = &summary.import_by {
        row(ui, "Accepted until", date);
    }
    egui::CollapsingHeader::new(
        egui::RichText::new("Details")
            .font(theme::medium(13.0))
            .color(palette.secondary),
    )
    .id_salt("chaat-setup-review-details")
    .default_open(false)
    .show(ui, |ui| {
        row(ui, "Application ID", &summary.client_id);
        row(ui, "Tenant", &summary.tenant);
        row(ui, "Created", &summary.created);
    })
    .header_response
    .on_hover_cursor(egui::CursorIcon::PointingHand);
    if warn_resign {
        ui.add_space(8.0);
        ui.label(
            egui::RichText::new("Applying signs every account in again.")
                .font(theme::regular(13.0))
                .color(palette.warning),
        );
    }
    ui.add_space(14.0);
    ui.horizontal(|ui| {
        let apply = if first_run {
            "  Continue to sign in  "
        } else {
            "Use these settings"
        };
        if widgets::primary_button(ui, &palette, apply).clicked() {
            app.actions.push(Action::ApplySetup);
        }
        if widgets::secondary_button(ui, &palette, "Cancel").clicked() {
            app.actions.push(Action::CancelSetup);
        }
    });
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.actions.push(Action::CancelSetup);
    }
}

const REVIEW_LABEL_WIDTH: f32 = 128.0;

/// Who the file lets sign in, in the person's terms.
fn who_can_sign_in(summary: &crate::setup_file::SetupSummary) -> String {
    if summary.expected_tenant.is_some() {
        format!("Only accounts from {}", summary.org_label)
    } else if summary.tenant == "organizations" {
        "Any work or school account".to_owned()
    } else {
        format!("Accounts in {}", summary.tenant)
    }
}

/// The host of a relay address, without scheme or path.
fn relay_host(url: &str) -> &str {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    rest.split(['/', '?', '#']).next().unwrap_or(rest)
}

#[cfg(test)]
mod tests {
    use super::relay_host;

    #[test]
    fn host_only() {
        assert_eq!(
            relay_host("https://relay.example.com/hook?x=1"),
            "relay.example.com"
        );
        assert_eq!(relay_host("relay.example.com"), "relay.example.com");
    }
}
