//! Drives the setup file screens (welcome step, import overlay, export form)
//! through real egui frames, without a window.

use std::path::PathBuf;

use chaat::app::{App, SetupStage};
use chaat::backend::{AuthStatus, Waker};
use chaat::model::{Action, SettingsPage};
use chaat::paths::AppDirs;
use chaat::settings::{Settings, ThemeChoice};
use chaat::setup_file::SetupSummary;

const GUID: &str = "11111111-2222-3333-4444-555555555555";

#[derive(Debug)]
struct Dropped(PathBuf);

impl egui::DroppedFile for Dropped {
    fn path(&self) -> &std::path::Path {
        &self.0
    }

    fn bytes(&self) -> Result<Vec<u8>, String> {
        Err("not readable".into())
    }
}

struct Harness {
    app: App,
    ctx: egui::Context,
    time: f64,
    size: egui::Vec2,
    shapes: Vec<egui::epaint::ClippedShape>,
    _dir: tempfile::TempDir,
}

fn flatten<'a>(shape: &'a egui::epaint::Shape, out: &mut Vec<&'a egui::epaint::TextShape>) {
    match shape {
        egui::epaint::Shape::Vec(shapes) => shapes.iter().for_each(|shape| flatten(shape, out)),
        egui::epaint::Shape::Text(text) => out.push(text),
        _ => {}
    }
}

impl Harness {
    /// The first-run welcome screen: nothing configured, no chats.
    fn welcome() -> Self {
        let mut h = Self::workspace();
        h.app.demo = false;
        h.app.chats.clear();
        h.app.auth = AuthStatus::NotConfigured;
        assert!(!h.app.shows_workspace());
        h
    }

    /// The demo workspace.
    fn workspace() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let app = App::new(
            AppDirs::under(dir.path()),
            Settings::default(),
            Waker::default(),
            true,
        );
        let ctx = egui::Context::default();
        chaat::ui::install(&ctx, true);
        Self {
            app,
            ctx,
            time: 0.0,
            size: egui::vec2(1280.0, 800.0),
            shapes: Vec::new(),
            _dir: dir,
        }
    }

    /// Settings > Advanced with demo mode off, so every group shows.
    fn advanced() -> Self {
        let mut h = Self::workspace();
        h.app.demo = false;
        h.app.settings.client_id = GUID.into();
        h.act(Action::ShowSettingsPage(SettingsPage::Advanced));
        h
    }

    fn frame_with(&mut self, events: Vec<egui::Event>, dropped: Vec<PathBuf>) {
        self.time += 1.0 / 60.0;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, self.size)),
            time: Some(self.time),
            events,
            dropped_files: dropped
                .into_iter()
                .map(|path| std::sync::Arc::new(Dropped(path)) as egui::DroppedFileHandle)
                .collect(),
            ..Default::default()
        };
        let app = &mut self.app;
        let mut output = self.ctx.run_ui(input, |ui| {
            let ctx = ui.ctx().clone();
            app.observe_window(&ctx);
            app.background(&ctx);
            chaat::ui::show(app, ui);
            app.apply_actions(&ctx);
        });
        output.textures_delta.clear();
        self.shapes = output.shapes;
    }

    fn frames(&mut self, count: usize) {
        for _ in 0..count {
            self.frame_with(Vec::new(), Vec::new());
        }
    }

    fn act(&mut self, action: Action) {
        self.app.actions.push(action);
        self.frames(3);
    }

    fn click(&mut self, pos: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        self.frame_with(
            vec![egui::Event::PointerMoved(pos), button(true)],
            Vec::new(),
        );
        self.frame_with(vec![button(false)], Vec::new());
        self.frames(3);
    }

    fn texts(&self) -> Vec<String> {
        let mut found = Vec::new();
        for clipped in &self.shapes {
            flatten(&clipped.shape, &mut found);
        }
        found
            .into_iter()
            .map(|text| text.galley.job.text.clone())
            .collect()
    }

    fn has_text(&self, wanted: &str) -> bool {
        self.texts().iter().any(|text| text == wanted)
    }

    fn text_pos(&self, wanted: &str) -> Option<egui::Pos2> {
        let mut found = Vec::new();
        for clipped in &self.shapes {
            flatten(&clipped.shape, &mut found);
        }
        found
            .into_iter()
            .find(|text| text.galley.job.text == wanted)
            .map(|text| text.pos + text.galley.rect.center().to_vec2())
    }

    /// How a text was painted: the colors egui applies at paint time and its
    /// opacity, which is where disabled widgets differ.
    fn text_look(&self, wanted: &str) -> Option<(egui::Color32, Option<egui::Color32>, f32)> {
        let mut found = Vec::new();
        for clipped in &self.shapes {
            flatten(&clipped.shape, &mut found);
        }
        found
            .into_iter()
            .find(|text| text.galley.job.text == wanted)
            .map(|text| {
                (
                    text.fallback_color,
                    text.override_text_color,
                    text.opacity_factor,
                )
            })
    }
}

fn summary(relay: bool) -> SetupSummary {
    SetupSummary {
        org_label: "Acme".into(),
        client_id: GUID.into(),
        tenant: "22222222-2222-3333-4444-555555555555".into(),
        expected_tenant: None,
        relay_url: relay.then(|| "https://relay.example.com".into()),
        relay_key_included: relay,
        created: "2026-11-21T10:00:00Z".into(),
        import_by: Some("2026-12-31".into()),
    }
}

fn stages() -> Vec<SetupStage> {
    let path = PathBuf::from("Acme.chaatsetup");
    vec![
        SetupStage::Idle,
        SetupStage::Passphrase(path.clone()),
        SetupStage::Opening(path),
        SetupStage::Review(summary(false)),
        SetupStage::Review(summary(true)),
        SetupStage::Applying,
        SetupStage::Failed("This file can't be read. Ask for a new one.".into()),
    ]
}

#[test]
fn every_stage_draws() {
    for size in [egui::vec2(720.0, 480.0), egui::vec2(1280.0, 800.0)] {
        for theme in [ThemeChoice::Dark, ThemeChoice::Light] {
            for stage in stages() {
                for manual in [false, true] {
                    let mut h = Harness::welcome();
                    h.size = size;
                    h.app.settings.theme = theme.clone();
                    h.app.setup.stage = stage.clone();
                    h.app.setup.manual = manual;
                    h.frames(3);
                    assert!(!h.texts().is_empty(), "{stage:?} at {size:?}");
                }
                // The import overlay on Settings > Advanced.
                let mut h = Harness::advanced();
                h.size = size;
                h.app.settings.theme = theme.clone();
                h.app.setup.stage = stage.clone();
                h.frames(3);
                assert!(h.has_text("SETUP FILE"), "{stage:?} at {size:?}");
            }
            // The export form and the passphrase reveal.
            for passphrase in [None, Some("one-two-three-four-five-six".to_owned())] {
                let mut h = Harness::advanced();
                h.size = size;
                h.app.settings.theme = theme.clone();
                h.app.settings.relay_url = Some("https://relay.example.com".into());
                h.app.settings.relay_key_set = true;
                h.act(Action::OpenExportForm(true));
                h.app.export.passphrase = passphrase;
                h.frames(3);
                assert!(h.has_text("SETUP FILE"));
            }
        }
    }
}

#[test]
fn drop_routes_to_setup() {
    let mut h = Harness::welcome();
    h.frames(2);
    h.frame_with(Vec::new(), vec![PathBuf::from("x.chaatsetup")]);
    h.frames(2);
    assert!(
        matches!(&h.app.setup.stage, SetupStage::Passphrase(path) if path.ends_with("x.chaatsetup")),
        "got {:?}",
        h.app.setup.stage
    );
    // In the workspace, with no import under way, the drop is an attachment.
    let mut h = Harness::workspace();
    h.frames(2);
    h.frame_with(Vec::new(), vec![PathBuf::from("x.chaatsetup")]);
    h.frames(2);
    assert_eq!(h.app.setup.stage, SetupStage::Idle);
}

#[test]
fn relay_checkbox_hidden_without_key() {
    const LABEL: &str = "Include the push relay (address and access key)";
    let mut h = Harness::advanced();
    h.app.settings.relay_url = Some("https://relay.example.com".into());
    h.app.settings.relay_key_set = false;
    h.act(Action::OpenExportForm(true));
    assert!(h.has_text("Organization name"));
    assert!(!h.has_text(LABEL));
    h.app.settings.relay_key_set = true;
    h.frames(2);
    assert!(h.has_text(LABEL));
}

#[test]
fn create_disabled_without_label() {
    let mut h = Harness::advanced();
    h.act(Action::OpenExportForm(true));
    h.app.export.org_label.clear();
    h.frames(2);
    let disabled = h.text_look("Create file…").expect("button drawn");
    h.app.export.org_label = "Acme".into();
    h.frames(2);
    let enabled = h.text_look("Create file…").expect("button drawn");
    assert_ne!(
        disabled, enabled,
        "the button looks disabled without a name"
    );
    // A click on the disabled button does nothing.
    h.app.export.org_label.clear();
    h.frames(2);
    let pos = h.text_pos("Create file…").unwrap();
    h.click(pos);
    assert!(h.app.export.error.is_none() && !h.app.export.busy);
}

#[test]
fn rail_collapses_narrow() {
    let mut h = Harness::welcome();
    h.size = egui::vec2(700.0, 480.0);
    h.frames(3);
    assert!(
        h.texts().iter().any(|t| t.starts_with("Step 1 of 3")),
        "{:?}",
        h.texts()
    );
    h.size = egui::vec2(1280.0, 800.0);
    h.frames(3);
    assert!(!h.texts().iter().any(|t| t.starts_with("Step 1 of 3")));
}

#[test]
fn manual_form_saves_registration() {
    let mut h = Harness::welcome();
    h.frames(2);
    let pos = h.text_pos("Enter details manually").expect("link drawn");
    h.click(pos);
    assert!(h.app.setup.manual);
    h.app.form_client_id = GUID.into();
    h.app.form_tenant = "organizations".into();
    h.frames(2);
    let pos = h.text_pos("Save").expect("Save drawn");
    h.click(pos);
    assert_eq!(h.app.settings.client_id, GUID);
    assert_eq!(h.app.settings.tenant, "organizations");
}

#[test]
fn failed_import_shows_message_and_a_way_forward() {
    let mut h = Harness::welcome();
    h.app.setup.stage = SetupStage::Failed("This file can't be read. Ask for a new one.".into());
    h.frames(3);
    assert!(h.has_text("This file can't be read. Ask for a new one."));
    assert!(h.has_text("Choose file"));
}

#[test]
fn manual_form_replaces_the_drop_zone() {
    let mut h = Harness::welcome();
    h.frames(2);
    assert!(h.has_text("Drop your setup file here"));
    h.app.setup.manual = true;
    h.frames(2);
    assert!(!h.has_text("Drop your setup file here"));
    assert!(h.has_text("Use a setup file instead"));
    assert!(h.has_text("Enter the application (client) ID to continue."));
}

#[test]
fn passphrase_can_be_shown_and_cancelled() {
    let mut h = Harness::welcome();
    h.app.setup.stage = SetupStage::Passphrase(PathBuf::from("Team.chaatsetup"));
    h.frames(2);
    assert!(
        h.has_text("Enter the six-word passphrase. It was sent to you separately from the file.")
    );
    let pos = h.text_pos("Show").expect("toggle drawn");
    h.click(pos);
    h.frames(2);
    assert!(h.has_text("Hide"));
    let pos = h.text_pos("Cancel").expect("Cancel drawn");
    h.click(pos);
    assert_eq!(h.app.setup.stage, SetupStage::Idle);
}

#[test]
fn review_on_first_run_continues_to_sign_in() {
    let mut h = Harness::welcome();
    h.app.setup.stage = SetupStage::Review(summary(true));
    h.frames(3);
    assert!(h.has_text("  Continue to sign in  "));
    assert!(h.has_text("Who can sign in"));
    // Only the rail names the step; the review has its own heading.
    assert!(h.has_text("Connect organization"));
    assert!(
        !h.has_text("Connect to your organization"),
        "one heading on the review"
    );
    // The IDs wait behind Details.
    assert!(!h.has_text(GUID));
}

#[test]
fn first_sign_in_offers_a_different_file() {
    let mut h = Harness::welcome();
    // First run: no account of the demo has ever signed in.
    for account in &mut h.app.accounts {
        account.auth = AuthStatus::SignedOut;
    }
    h.app.settings.accounts.clear();
    h.frames(3);
    assert!(h.has_text("Use a different setup file"));
    // The optional note shows from step two on.
    assert!(h.has_text("Optional. Set up later in Settings › Push updates."));
}

#[test]
fn forced_welcome_looks_like_a_first_run() {
    let mut first = Harness::welcome();
    first.frames(3);
    let mut forced = Harness::workspace();
    forced.app.onboarding_forced = true;
    forced.frames(3);
    assert!(!forced.app.shows_workspace());
    // Same screen as a real first run: no extra text for the variable.
    assert_eq!(forced.texts(), first.texts());
}

#[test]
fn applying_shows_progress_on_welcome() {
    let mut h = Harness::welcome();
    h.app.setup.stage = SetupStage::Applying;
    h.frames(3);
    assert!(h.has_text("Applying…"));
}

#[test]
fn import_overlay_survives_leaving_settings() {
    let mut h = Harness::advanced();
    h.app.setup.stage = SetupStage::Applying;
    h.act(Action::ShowSettingsPage(SettingsPage::General));
    h.frames(2);
    assert!(h.has_text("Applying…"));
}
